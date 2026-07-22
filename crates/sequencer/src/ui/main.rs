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
mod reactive_sync;

use agent_finalize::*;
use edit_sessions::*;
use history_commands::*;
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
    // Pointer-rate rack edits update the live graph immediately. Their large
    // immutable scheduler snapshot is committed once at gesture end instead
    // of being rebuilt for every mouse pixel.
    let mut rack_control_snapshot_dirty = false;
    let mut sample_import_session: Option<SampleImportSession> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;
    let mut soft_step_param_edit = SoftStepParamEdit::default();
    let mut piano_roll_history_gesture: Option<ActivePianoRollHistoryGesture> = None;
    let mut lisp_hot_reload_watcher = LispHotReloadWatcher::start(watched_lisp_paths(&editor));
    let mut lisp_hot_reload_source_revision = editor.runtime().lisp_source_revision();
    let mut last_lisp_hot_reload_path_scan = Instant::now();

    // Inline editor session state (instrument/effect creation/editing)
    let mut editor_buffer_name: Option<String> = None;
    let mut editor_mode: Option<String> = None;
    let mut instrument_edit_session: Option<InstrumentEditSession> = None;
    let mut pending_instrument_preview: Option<PendingInstrumentPreview> = None;
    let mut pending_instrument_cancel_restore: Option<PendingInstrumentCancelRestore> = None;
    let mut pending_saved_instrument_load: Option<PendingSavedInstrumentLoad> = None;
    let mut pending_key_lock_auditions: Vec<PendingKeyLockAudition> = Vec::new();
    let mut effect_edit_session: Option<EffectEditSession> = None;
    let mut pending_effect_preview: Option<PendingEffectPreview> = None;
    let mut pending_effect_cancel_restore: Option<PendingEffectCancelRestore> = None;
    let mut script_draft_session: Option<ScriptDraftSession> = None;
    let mut pending_agentic_bubbles: HashMap<String, PendingAgenticBubble> = HashMap::new();
    let mut pending_lisp_history_transactions = HashMap::new();
    let mut prev_editor_macro_action: (String, String) = (String::new(), String::new());
    let mut prev_playing = false;
    let mut prev_bpm: u32 = 0;
    let mut prev_playhead: u32 = u32::MAX;
    let mut prev_transport_playhead: u32 = u32::MAX;
    let mut prev_pattern_epoch: u64 = 0;
    let mut prev_current_track: usize = usize::MAX;
    let mut prev_cpu_load_bits: u32 = u32::MAX;
    let mut prev_peak_l_level = -1.0f64;
    let mut prev_peak_r_level = -1.0f64;
    let mut prev_recording = false;
    let mut prev_master_recording = false;
    let mut prev_selected_tracks: HashSet<usize> = HashSet::new();
    let mut prev_groups: Vec<sequencer::project::ProjectTrackGroup> = Vec::new();
    let mut prev_track_peak_levels: Vec<f64> = Vec::new();
    let mut prev_rack_slot_peak_levels: Vec<Vec<f64>> = Vec::new();
    let mut prev_bus_peak_levels: Vec<f64> = Vec::new();
    let mut prev_modulator_phases: Vec<f64> = Vec::new();
    let mut prev_modulator_levels: Vec<f64> = Vec::new();
    let mut prev_bus_playheads: Vec<usize> = Vec::new();
    let mut prev_track_playheads: Vec<u32> = Vec::new();
    let mut prev_track_button_states = track_button_state_snapshot(&state);
    let mut prev_current_track_playhead_visible = false;
    let mut prev_ui_epoch: usize = 0;
    let mut prev_fx_epoch: usize = 0;
    let mut prev_instrument_active_notes: Vec<u8> = Vec::new();
    let mut preview_plock_variant: Option<(usize, String)> = None;
    let mut prev_active_buffer_name = editor.active_buffer().name.clone();
    let mut prev_selected_neural_neurons = selected_neural_neurons.lock().unwrap().clone();
    let mut prev_agent_generation_watermark = agent_generation_watermark(&app);
    let mut prev_sampler_analysis_key: Option<(usize, i32, u32, u32, usize)> = None;
    let mut prev_auto_follow = true;
    let mut watched_sampler_voice_track: Option<usize> = None;
    let mut watched_sampler_voice_ids: Vec<i32> = Vec::new();
    let mut cached_peak_l_level = 0.0f64;
    let mut cached_peak_r_level = 0.0f64;
    let mut cached_track_peak_levels = vec![0.0; track_names.len()];
    let mut cached_rack_slot_peak_levels: Vec<Vec<f64>> = Vec::new();
    let mut cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
    let mut prev_queued_transport_scene: Option<usize> = None;
    let (mut cached_modulator_phases, mut cached_modulator_levels) =
        read_modulator_display_values(app.graph.lg, &app);
    let mut last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL;
    let mut live_audio_analyzer = LiveAudioAnalyzerManager::new(app.graph.lg);
    let mut last_neural_visualization_poll_at = Instant::now() - NEURAL_VISUALIZATION_POLL_INTERVAL;
    let mut last_cpu_ui_poll_at = Instant::now() - CPU_UI_POLL_INTERVAL;
    let mut last_voice_count_log_at = Instant::now() - VOICE_COUNT_LOG_INTERVAL;
    let log_voice_counts = std::env::var_os("TINYSEQ_LOG_VOICE_COUNTS").is_some();
    if log_voice_counts {
        sequencer::voice_modulator::set_process_stats_enabled(true);
    }
    let mut cached_cpu_load_bits: u32 = 0.0f32.to_bits();

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
        if queued_transport_scene != prev_queued_transport_scene {
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
            prev_queued_transport_scene = queued_transport_scene;
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
            &mut pending_key_lock_auditions,
            &keyboard_tx,
            Instant::now(),
        );
        pull_shared_bus_state(&mut app, &bus_state);
        let recording_now = recording.load(Ordering::Relaxed);
        if recording_now != prev_recording {
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
                prev_recording = false;
            } else {
                prev_recording = recording_now;
            }
        }
        if !app.has_pending_project_load() {
            pull_named_scratch_buffer_into_project(&editor, &mut app);
        }
        editor.update_timers();
        let active_buffer_name = editor.active_buffer().name.clone();
        if active_buffer_name != prev_active_buffer_name {
            prev_active_buffer_name = active_buffer_name;
            let mut guard = active_delete_target.lock().unwrap();
            let should_clear = should_clear_active_delete_target_for_buffer(
                guard.as_ref(),
                &prev_active_buffer_name,
            );
            if should_clear {
                guard.take();
                drop(guard);
                active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        let agent_generation = agent_generation_watermark(&app);
        if agent_generation != prev_agent_generation_watermark {
            eprintln!(
                "[agent-ui] generation changed {} -> {}; refreshing *agent*",
                prev_agent_generation_watermark, agent_generation
            );
            prev_agent_generation_watermark = agent_generation;
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
        if log_voice_counts && last_voice_count_log_at.elapsed() >= VOICE_COUNT_LOG_INTERVAL {
            log_active_voice_counts(&state, &track_names);
            last_voice_count_log_at = Instant::now();
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
                        if let Some(gesture) = piano_roll_history_gesture.take() {
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
                            prev_recording = false;
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
                                    cached_track_peak_levels = read_track_peak_levels(
                                        app.graph.lg,
                                        &track_pan_ids.lock().unwrap(),
                                    );
                                    cached_bus_peak_levels =
                                        read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                                    (cached_modulator_phases, cached_modulator_levels) =
                                        read_modulator_display_values(app.graph.lg, &app);
                                    last_meter_poll_at = Instant::now();
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
                                        &cached_track_peak_levels,
                                    );
                                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                                    sync_modulator_phase_fields(rt, &cached_modulator_phases);
                                    sync_modulator_level_fields(rt, &cached_modulator_levels);
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
                                    prev_track_playheads = track_playheads_snapshot(&state, &app);
                                    prev_track_button_states = track_button_state_snapshot(&state);
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
                            prev_selected_neural_neurons = selection;
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
                    pending_lisp_history_transactions.insert(
                        id,
                        (label, app.history.clone(), app.history.undo_len()),
                    );
                    continue;
                }
                HostCommand::AuthoringTransactionEnd { id, success } => {
                    if let Some((label, checkpoint, checkpoint_len)) =
                        pending_lisp_history_transactions.remove(&id)
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
                match name.as_str() {
                    "midi-fx-history-action" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "MIDI FX edit failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
                        let op = field("op").and_then(|value| match value {
                            Value::Keyword(value) | Value::String(value) | Value::Symbol(value) => Some(value),
                            _ => None,
                        });
                        let track = field("track").and_then(|value| match value {
                            Value::Number(value) if value >= 0.0 => Some(value as usize),
                            _ => None,
                        });
                        let (Some(op), Some(track)) = (op, track) else {
                            editor.handle_host_event(HostEvent::Error(
                                "MIDI FX edit failed: missing target".to_string(),
                            ));
                            continue;
                        };
                        enum MidiFxHistoryMutation {
                            Chain(Vec<String>),
                            Position(sequencer::sequencer::MidiFxPosition),
                        }
                        let mutation = match op.as_str() {
                            "set-chain" => match field("value") {
                                Some(Value::List(values)) => {
                                    let chain = values.into_iter().map(|value| {
                                        match &*value.borrow() {
                                            Value::String(name) => Ok(name.clone()),
                                            _ => Err("MIDI FX chain contains a non-string name".to_string()),
                                        }
                                    }).collect::<Result<Vec<_>, _>>();
                                    match chain {
                                        Ok(chain) => MidiFxHistoryMutation::Chain(chain),
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(error));
                                            continue;
                                        }
                                    }
                                }
                                _ => {
                                    editor.handle_host_event(HostEvent::Error(
                                        "MIDI FX chain is missing".to_string(),
                                    ));
                                    continue;
                                }
                            },
                            "set-position" => match field("value") {
                                Some(Value::Keyword(value)) | Some(Value::String(value))
                                    if value == "post-accumulator" =>
                                {
                                    MidiFxHistoryMutation::Position(
                                        sequencer::sequencer::MidiFxPosition::PostAccumulator,
                                    )
                                }
                                Some(Value::Keyword(value)) | Some(Value::String(value))
                                    if value == "pre-accumulator" =>
                                {
                                    MidiFxHistoryMutation::Position(
                                        sequencer::sequencer::MidiFxPosition::PreAccumulator,
                                    )
                                }
                                _ => {
                                    editor.handle_host_event(HostEvent::Error(
                                        "MIDI FX position is invalid".to_string(),
                                    ));
                                    continue;
                                }
                            },
                            _ => {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Unknown MIDI FX edit {op}"
                                )));
                                continue;
                            }
                        };
                        let Some(params) = app.state.pattern.track_params.get(track) else {
                            editor.handle_host_event(HostEvent::Error(
                                "MIDI FX track no longer exists".to_string(),
                            ));
                            continue;
                        };
                        let unchanged = match &mutation {
                            MidiFxHistoryMutation::Chain(chain) => params.midi_fx_chain() == *chain,
                            MidiFxHistoryMutation::Position(position) => {
                                params.get_midi_fx_position() == *position
                            }
                        };
                        if unchanged {
                            continue;
                        }
                        let result = app.apply_recorded_scene_structure_mutation(
                            "Edit MIDI FX routing",
                            |app| {
                                let params = app.state.pattern.track_params.get(track)
                                    .ok_or_else(|| "MIDI FX track no longer exists".to_string())?;
                                match mutation {
                                    MidiFxHistoryMutation::Chain(chain) => {
                                        params.set_midi_fx_chain(chain)
                                    }
                                    MidiFxHistoryMutation::Position(position) => {
                                        params.set_midi_fx_position(position)
                                    }
                                }
                                Ok(())
                            },
                        );
                        match result {
                            Ok(()) => {
                                state.publish_scheduler_snapshot();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                                "MIDI FX edit failed: {error}"
                            ))),
                        }
                    }
                    "process-history-action" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Process edit failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
                        let op = field("op").and_then(|value| match value {
                            Value::Keyword(value) | Value::String(value) | Value::Symbol(value) => Some(value),
                            _ => None,
                        });
                        let track = field("track").and_then(|value| match value {
                            Value::Number(value) if value >= 0.0 => Some(value as usize),
                            _ => None,
                        });
                        let instance_id = field("instance-id").and_then(|value| match value {
                            Value::Number(value) if value >= 0.0 => {
                                Some(sequencer::process::ProcessInstanceId(value as u64))
                            }
                            _ => None,
                        });
                        let (Some(op), Some(track), Some(instance_id)) =
                            (op, track, instance_id)
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Process edit failed: missing operation target".to_string(),
                            ));
                            continue;
                        };
                        let result = app.apply_recorded_scene_structure_mutation(
                            "Edit process chain",
                            |app| {
                                let changed = match op.as_str() {
                                    "set-lane-step" => {
                                        let inlet = field("inlet").and_then(|value| match value {
                                            Value::String(value) => Some(value),
                                            _ => None,
                                        }).ok_or_else(|| "Process lane inlet is missing".to_string())?;
                                        let step = field("step").and_then(|value| match value {
                                            Value::Number(value) if value >= 0.0 => Some(value as usize),
                                            _ => None,
                                        }).ok_or_else(|| "Process lane step is missing".to_string())?;
                                        let value = field("value").and_then(|value| match value {
                                            Value::Number(value) => Some(value as f32),
                                            _ => None,
                                        }).ok_or_else(|| "Process lane value is missing".to_string())?;
                                        app.state.set_process_lane_value(
                                            track, instance_id, inlet, step, value,
                                        )
                                    }
                                    "clear-project-lane-override" => {
                                        let inlet = field("inlet").and_then(|value| match value {
                                            Value::String(value) => Some(value),
                                            _ => None,
                                        }).ok_or_else(|| "Process lane inlet is missing".to_string())?;
                                        app.state.clear_project_process_lane_override(
                                            track, instance_id, &inlet,
                                        )
                                    }
                                    "set-inlet" => {
                                        let inlet = field("inlet").and_then(|value| match value {
                                            Value::String(value) => Some(value),
                                            _ => None,
                                        }).ok_or_else(|| "Process inlet is missing".to_string())?;
                                        let literal = match field("value")
                                            .ok_or_else(|| "Process inlet value is missing".to_string())?
                                        {
                                            Value::Number(value) => sequencer::process::ProcessLiteral::Number(value),
                                            Value::Bool(value) => sequencer::process::ProcessLiteral::Bool(value),
                                            Value::String(value) => sequencer::process::ProcessLiteral::String(value),
                                            Value::Keyword(value) => sequencer::process::ProcessLiteral::Keyword(value),
                                            Value::Symbol(value) => sequencer::process::ProcessLiteral::Symbol(value),
                                            Value::Nil => sequencer::process::ProcessLiteral::Nil,
                                            _ => return Err("Unsupported process inlet literal".to_string()),
                                        };
                                        app.state.set_track_process_inlet_value(
                                            track, instance_id, &inlet, literal,
                                        )
                                    }
                                    "set-enabled" => {
                                        let enabled = field("enabled").and_then(|value| match value {
                                            Value::Bool(value) => Some(value),
                                            _ => None,
                                        }).ok_or_else(|| "Process enabled state is missing".to_string())?;
                                        app.state.set_track_process_slot_enabled(
                                            track, instance_id, enabled,
                                        )
                                    }
                                    "move-slot" => {
                                        let before = match field("before-instance-id") {
                                            Some(Value::Number(value)) if value >= 0.0 => {
                                                Some(sequencer::process::ProcessInstanceId(value as u64))
                                            }
                                            Some(Value::Nil) | None => None,
                                            _ => return Err("Process move target is invalid".to_string()),
                                        };
                                        app.state.move_track_process_slot_before(
                                            track, instance_id, before,
                                        )
                                    }
                                    "remove-slot" => app.state.remove_track_process_slot(
                                        track, instance_id,
                                    ),
                                    "bind-port" => {
                                        let port = field("port").and_then(|value| match value {
                                            Value::String(value) => Some(value),
                                            _ => None,
                                        }).ok_or_else(|| "Process port is missing".to_string())?;
                                        let target = field("target")
                                            .ok_or_else(|| "Process binding target is missing".to_string())?;
                                        let target = natives::param_target_from_value(
                                            &app.state, track, &target,
                                        )?;
                                        app.state.set_process_port_binding(
                                            track, instance_id, &port, target,
                                        )
                                    }
                                    "clear-port-binding" => {
                                        let port = field("port").and_then(|value| match value {
                                            Value::String(value) => Some(value),
                                            _ => None,
                                        }).ok_or_else(|| "Process port is missing".to_string())?;
                                        app.state.clear_process_port_binding(
                                            track, instance_id, &port,
                                        )
                                    }
                                    _ => return Err(format!("Unknown process history operation {op}")),
                                };
                                changed.then_some(()).ok_or_else(|| {
                                    "Process edit target was missing or unchanged".to_string()
                                })
                            },
                        );
                        match result {
                            Ok(()) => ui_invalidations.push(UiInvalidation::ProcessChain { track }),
                            Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                "Process edit failed: {error}"
                            ))),
                        }
                    }
                    "piano-roll-gesture-update" => {
                        match apply_piano_roll_gesture_update(
                            &mut app,
                            &piano_roll_selection,
                            &piano_roll_move_state,
                            &mut piano_roll_history_gesture,
                            &payload,
                        ) {
                            Ok((status, track)) => {
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                ui_invalidations.push(UiInvalidation::PianoRoll {
                                    track,
                                    change: PianoRollInvalidation::Items,
                                });
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_transient_message(status);
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "piano-roll-gesture-finish" => {
                        match finish_piano_roll_gesture(
                            &mut app,
                            &piano_roll_move_state,
                            &mut piano_roll_history_gesture,
                            &payload,
                        ) {
                            Ok((app::edit::EditOutcome::Applied(result), track)) => {
                                ui_invalidations.push(UiInvalidation::PianoRoll {
                                    track,
                                    change: PianoRollInvalidation::Items,
                                });
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, _)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Piano-roll gesture was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "piano-roll-history-action" => {
                        match apply_piano_roll_history_host_command(
                            &mut app,
                            &piano_roll_selection,
                            &piano_roll_move_state,
                            &piano_roll_clipboard,
                            &payload,
                        ) {
                            Ok((outcome, status, track)) => {
                                if matches!(outcome, app::edit::EditOutcome::Applied(_)) {
                                    *auto_follow_override_until.lock().unwrap() =
                                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                    ui_invalidations.push(UiInvalidation::PianoRoll {
                                        track,
                                        change: PianoRollInvalidation::Items,
                                    });
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                                editor.show_transient_message(status);
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "drum-lane-history-action" => {
                        match apply_drum_lane_history_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), action)) => {
                                let track = action.track();
                                let bindings = editor.runtime().reactive_binding_store();
                                match action {
                                    DrumLaneHistoryAction::Toggle { .. } => {
                                        if !selected_steps.lock().unwrap().is_empty() {
                                            selected_steps.lock().unwrap().clear();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        clear_drum_lane_selection(
                                            &bindings,
                                            &mut selected_drum_lane_steps.lock().unwrap(),
                                        );
                                    }
                                    DrumLaneHistoryAction::Move {
                                        pad_note,
                                        steps,
                                        delta,
                                        move_selection: true,
                                        ..
                                    } => {
                                        let mut selected = selected_drum_lane_steps.lock().unwrap();
                                        for step in steps {
                                            let old = DrumLaneStepSelection {
                                                track,
                                                pad_note,
                                                step,
                                            };
                                            selected.remove(&old);
                                            write_drum_lane_selection(&bindings, old, false);
                                            let new = DrumLaneStepSelection {
                                                track,
                                                pad_note,
                                                step: (step as isize + delta) as usize,
                                            };
                                            selected.insert(new);
                                            write_drum_lane_selection(&bindings, new, true);
                                        }
                                    }
                                    DrumLaneHistoryAction::Clear { .. } => {
                                        clear_drum_lane_selection(
                                            &bindings,
                                            &mut selected_drum_lane_steps.lock().unwrap(),
                                        );
                                    }
                                    DrumLaneHistoryAction::Duration { .. }
                                    | DrumLaneHistoryAction::Move {
                                        move_selection: false,
                                        ..
                                    } => {}
                                }
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                ui_invalidations.push(UiInvalidation::Pattern(
                                    PatternInvalidation::WholeTrack { track },
                                ));
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, _)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Drum-lane edit was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "delete-selected-steps" => {
                        let track = match &payload {
                            Value::Map(map) => map_usize(map, "track"),
                            _ => None,
                        };
                        let Some(track) = track else {
                            editor.handle_host_event(HostEvent::Error(
                                "Selected-step delete target was invalid".to_string(),
                            ));
                            continue;
                        };
                        match apply_selected_steps_delete(
                            &mut app,
                            track,
                            &selected_steps,
                        ) {
                            Ok((app::edit::EditOutcome::Applied(_), steps)) => {
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                ui_invalidations.push(UiInvalidation::Pattern(
                                    PatternInvalidation::WholeTrack { track },
                                ));
                                ui_invalidations.push(UiInvalidation::StepSelection {
                                    track,
                                    changed_steps: steps,
                                });
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(_) => {}
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "paste-steps" => {
                        match apply_step_paste_host_command(&mut app, &step_clipboard, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), track)) => {
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                ui_invalidations.push(UiInvalidation::Pattern(
                                    PatternInvalidation::WholeTrack { track },
                                ));
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, _)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Step paste was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "set-step-param-history" => {
                        match apply_step_param_history_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), track, steps, param)) => {
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                for step in steps {
                                    ui_invalidations.push(UiInvalidation::Step {
                                        track,
                                        step,
                                        change: StepInvalidation::Param(param.into()),
                                    });
                                    if param == StepParam::Duration {
                                        ui_invalidations.push(UiInvalidation::Step {
                                            track,
                                            step,
                                            change: StepInvalidation::DurationSpan,
                                        });
                                    }
                                }
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, ..)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, ..)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Step parameter edit was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "move-step-history" => {
                        match apply_move_step_history_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), track, steps, affected_steps, delta, move_selection)) => {
                                let moved_steps = steps
                                    .iter()
                                    .map(|step| (*step as isize + delta) as usize)
                                    .collect::<Vec<_>>();
                                let mut changed_selection = Vec::new();
                                if move_selection {
                                    let mut selected = selected_steps.lock().unwrap();
                                    let previous = selected.clone();
                                    selected.clear();
                                    selected.extend(moved_steps.iter().copied());
                                    changed_selection = previous
                                        .symmetric_difference(&selected)
                                        .copied()
                                        .collect();
                                    changed_selection.sort_unstable();
                                }
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                ui_invalidations.push(UiInvalidation::StepBatch {
                                    track,
                                    steps: affected_steps,
                                });
                                if move_selection {
                                    ui_invalidations.push(UiInvalidation::StepSelection {
                                        track,
                                        changed_steps: changed_selection,
                                    });
                                }
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, ..)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, ..)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Step move was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "slice2-history-action" => {
                        match apply_slice2_history_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), track)) => {
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                ui_invalidations.push(UiInvalidation::Pattern(
                                    PatternInvalidation::WholeTrack { track },
                                ));
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, _)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Slice 2 edit was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "slice3-history-action" => {
                        match apply_slice3_history_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), track)) => {
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                match (track, slice3_track_mixer_invalidation(&payload)) {
                                    (Some(track), Some(change)) => {
                                        ui_invalidations
                                            .push(UiInvalidation::TrackMixer { track, change });
                                    }
                                    (track, None) => {
                                        if let Some(track) = track {
                                            ui_invalidations.push(UiInvalidation::Pattern(
                                                PatternInvalidation::WholeTrack { track },
                                            ));
                                        }
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    (None, Some(_)) => {
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, _)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Slice 3 edit was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "bus-mixer-history-action" => {
                        match apply_bus_mixer_history_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(result), bus)) => {
                                *bus_state.lock().unwrap() = app.buses.clone();
                                match bus_mixer_targeted_invalidation(&payload) {
                                    Some(change) => {
                                        ui_invalidations
                                            .push(UiInvalidation::BusMixer { bus, change });
                                    }
                                    None => {
                                        ui_invalidations.push(UiInvalidation::BusMixer {
                                            bus,
                                            change: BusMixerInvalidation::Volume,
                                        });
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                editor.show_transient_message(result.label);
                            }
                            Ok((app::edit::EditOutcome::NoOp, _)) => {}
                            Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Bus mixer edit was applied without history".to_string(),
                                ));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "toggle-step" => {
                        match apply_toggle_step_host_command(&mut app, &payload) {
                            Ok((app::edit::EditOutcome::Applied(_), track, step)) => {
                                let mut selection = selected_steps.lock().unwrap();
                                if !selection.is_empty() {
                                    selection.clear();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                                drop(selection);
                                *auto_follow_override_until.lock().unwrap() =
                                    Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                                // The targeted Step invalidations were the
                                // complete pre-undo UI path for toggles; no
                                // ui_epoch bump so fast toggle-drags skip the
                                // full resync per step.
                                ui_invalidations.push(UiInvalidation::StepBatch {
                                    track,
                                    steps: vec![step],
                                });
                            }
                            Ok(_) => {}
                            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                        }
                    }
                    "set-scene-launch-quantize" => {
                        let Value::String(label) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "Scene launch quantization selection was invalid".to_string(),
                            ));
                            continue;
                        };
                        let Some(quantize) =
                            sequencer::quantized_launch::LaunchQuantize::from_transport_label(
                                &label,
                            )
                        else {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Unknown scene launch quantization: {label}"
                            )));
                            continue;
                        };
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "scene-launch-quantize",
                            Value::String(quantize.transport_label().to_string()),
                        );
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                    }
                    "set-record-quantize" => {
                        let Value::String(label) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "Record quantization selection was invalid".to_string(),
                            ));
                            continue;
                        };
                        let Some(quantize) =
                            sequencer::record_quantize::RecordQuantize::from_transport_label(
                                &label,
                            )
                        else {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Unknown record quantization: {label}"
                            )));
                            continue;
                        };
                        state
                            .transport
                            .record_quantize
                            .store(quantize as u32, Ordering::Release);
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "record-quantize",
                            Value::String(quantize.transport_label().to_string()),
                        );
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                    }
                    "toggle-metronome" => {
                        let enabled = !state
                            .transport
                            .metronome_enabled
                            .fetch_xor(true, Ordering::AcqRel);
                        editor
                            .runtime_mut()
                            .set_reactive("SEQ", "metronome", Value::Bool(enabled));
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                    }
                    "reveal-sequencer-track" => {
                        if let Some(track) = extract_usize_from_payload(&payload, "track") {
                            if track < app.tracks.len() {
                                reveal_sequencer_current_track(&mut editor, &app, track);
                            }
                        }
                    }
                    "audition-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        eprintln!(
                            "sample-host-command: audition-sample payload={payload:?}; extracted_path={path_str:?}"
                        );
                        if let Some(path_str) = path_str {
                            if app.tracks.is_empty() {
                                editor.handle_host_event(HostEvent::Status(
                                    "Add a track before auditioning samples".to_string(),
                                ));
                                continue;
                            }
                            let path = Path::new(&path_str);
                            let Some(track) = current_track_for_app(&mut app, &current_track)
                            else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Add a track before auditioning samples".to_string(),
                                ));
                                continue;
                            };
                            match load_or_convert_sampler_track(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &selected_steps,
                                lg_raw,
                                track,
                                Some(path),
                            ) {
                                Ok(result) => {
                                    let status = result.reset_summary.map_or_else(
                                        || format!("Audition: {}", result.name),
                                        |summary| {
                                            host_commands::instrument_swap_status(
                                                "sampler", summary,
                                            )
                                        },
                                    );
                                    editor.handle_host_event(HostEvent::Status(status));
                                }
                                Err(e) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error loading sample: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-sampler" => match app.graph_controller().add_blank_sampler_track()
                        .and_then(|idx| {
                            app.commit_created_track(idx, "Add sampler track")?;
                            Ok(idx)
                        }) {
                        Ok(idx) => {
                            current_track.store(idx, Ordering::Relaxed);
                            let new_name = app.tracks[idx].clone();
                            track_names.push(new_name.clone());
                            {
                                let mut pan_ids = track_pan_ids.lock().unwrap();
                                pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                                push_solo_mutes(lg_raw, &state, &pan_ids);
                            }
                            record_armed.lock().unwrap().push(false);
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "num-tracks",
                                Value::Number(track_names.len() as f64),
                            );
                            rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                            set_current_track_reactive(rt, app.tracks.len(), idx);
                            rt.set_reactive("SEQ", "track-names", build_track_names(&track_names));
                            sync_all_track_sequencer_state(rt, &state, &app, idx, &selected_steps);
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, idx));
                            sync_step_param_lists(rt, &state, idx);
                            sync_track_mixer_state(rt, &app, &state);
                            sync_bus_mixer_state(rt, &app);
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                            rt.set_reactive(
                                "SEQ",
                                "effects",
                                build_effects_value(
                                    &state,
                                    idx,
                                    &app.graph.effect_descriptors,
                                    &selected_steps,
                                ),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "midi-effects",
                                build_midi_effects_value(&state, idx, &selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, idx, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            let selected_neural_snapshot =
                                selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                idx,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                idx,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(&state, idx, &app.graph.effect_descriptors),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added sampler track {}: {new_name}",
                                idx + 1
                            )));
                        }
                        Err(e) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error adding sampler track: {e}"
                            )));
                        }
                    },
                    "add-track-rack" => {
                        let path_str = extract_path_from_payload(&payload)
                            .filter(|path| !path.trim().is_empty());
                        let result = if let Some(path_str) = path_str {
                            let path = Path::new(&path_str);
                            app.graph_controller().add_sampler_drum_rack_track(
                                path,
                                sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE,
                            )
                        } else {
                            app.graph_controller().add_empty_rack_track()
                        };
                        let result = result.and_then(|idx| {
                            app.commit_created_track(idx, "Add drum rack track")?;
                            Ok(idx)
                        });
                        match result {
                            Ok(idx) => {
                                sync_after_instrument_track_apply(
                                    &mut app,
                                    &mut editor,
                                    &state,
                                    idx,
                                    &current_track,
                                    &mut track_names,
                                    &track_pan_ids,
                                    &record_armed,
                                    &selected_steps,
                                    &accumulator_names,
                                    &cached_track_peak_levels,
                                    &cached_bus_peak_levels,
                                    &ui_epoch,
                                    lg_raw,
                                );
                                let new_name = app.tracks[idx].clone();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Added drum rack track {}: {new_name}",
                                    idx + 1
                                )));
                            }
                            Err(e) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding drum rack track: {e}"
                                )));
                            }
                        }
                    }
                    "add-track-layer-rack" => {
                        let path_str = extract_path_from_payload(&payload)
                            .filter(|path| !path.trim().is_empty());
                        let result = if let Some(path_str) = path_str {
                            let path = PathBuf::from(path_str);
                            app.graph_controller().add_sampler_rack_track(&[path])
                        } else {
                            app.graph_controller().add_empty_layer_rack_track()
                        };
                        let result = result.and_then(|idx| {
                            app.commit_created_track(idx, "Add layer rack track")?;
                            Ok(idx)
                        });
                        match result {
                            Ok(idx) => {
                                sync_after_instrument_track_apply(
                                    &mut app,
                                    &mut editor,
                                    &state,
                                    idx,
                                    &current_track,
                                    &mut track_names,
                                    &track_pan_ids,
                                    &record_armed,
                                    &selected_steps,
                                    &accumulator_names,
                                    &cached_track_peak_levels,
                                    &cached_bus_peak_levels,
                                    &ui_epoch,
                                    lg_raw,
                                );
                                let new_name = app.tracks[idx].clone();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Added layer rack track {}: {new_name}",
                                    idx + 1
                                )));
                            }
                            Err(e) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding layer rack track: {e}"
                                )));
                            }
                        }
                    }
                    "add-track-rack-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        match path_str {
                            Some(path_str) => {
                                let path = PathBuf::from(path_str);
                                match app.graph_controller().add_sampler_drum_rack_track(
                                    &path,
                                    sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE,
                                ).and_then(|idx| {
                                    app.commit_created_track(idx, "Add drum rack track")?;
                                    Ok(idx)
                                }) {
                                    Ok(idx) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            idx,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        let new_name = app.tracks[idx].clone();
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Added drum rack track {}: {new_name}",
                                            idx + 1
                                        )));
                                    }
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error adding drum rack track: {e}"
                                        )));
                                    }
                                }
                            }
                            None => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Drum rack track creation is missing a sample path".to_string(),
                                ));
                            }
                        }
                    }
                    "add-rack-sample-slot" => {
                        let path_str = extract_path_from_payload(&payload);
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        match (track, path_str) {
                            (Some(track), Some(path_str)) => {
                                if preserve_browser_context {
                                    preserve_sample_browser_context_for_loaded_sample(
                                        &mut editor,
                                        &path_str,
                                    );
                                }
                                let path = Path::new(&path_str);
                                match app.apply_recorded_rack_slot_add(
                                    track,
                                    "Add rack sample",
                                    |app| app.graph_controller().add_sampler_slot_to_rack(track, path),
                                ) {
                                    Ok(slot_idx) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Added rack layer {}",
                                            slot_idx + 1
                                        )));
                                    }
                                    Err(e) => {
                                        if preserve_browser_context {
                                            preserve_sample_browser_context_for_loaded_sample(
                                                &mut editor,
                                                "",
                                            );
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error adding rack layer: {e}"
                                        )));
                                    }
                                }
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Rack layer is missing a track or sample path".to_string(),
                                ));
                            }
                        }
                    }
                    "replace-rack-slot-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        match (track, slot, path_str) {
                            (Some(track), Some(slot), Some(path_str)) => {
                                if preserve_browser_context {
                                    preserve_sample_browser_context_for_loaded_sample(
                                        &mut editor,
                                        &path_str,
                                    );
                                }
                                match app.apply_recorded_rack_slot_source_replacement(
                                    track,
                                    slot,
                                    "Replace rack sample",
                                    |app| app.graph_controller().replace_rack_slot_with_sampler(
                                        track,
                                        slot,
                                        Path::new(&path_str),
                                    ),
                                ) {
                                    Ok(()) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Replaced rack layer {} with sample",
                                            slot + 1
                                        )));
                                    }
                                    Err(error) => {
                                        if preserve_browser_context {
                                            preserve_sample_browser_context_for_loaded_sample(
                                                &mut editor,
                                                "",
                                            );
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error replacing rack layer: {error}"
                                        )));
                                    }
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack layer replacement is missing a track, slot, or sample path"
                                    .to_string(),
                            )),
                        }
                    }
                    "add-rack-sample-pad" => {
                        let path_str = extract_path_from_payload(&payload);
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let pad_note = extract_i32_from_payload(&payload, "pad-note");
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        match (track, pad_note, path_str) {
                            (Some(track), Some(pad_note), Some(path_str)) => {
                                if preserve_browser_context {
                                    preserve_sample_browser_context_for_loaded_sample(
                                        &mut editor,
                                        &path_str,
                                    );
                                }
                                let path = Path::new(&path_str);
                                match app
                                    .graph_controller()
                                    .add_sampler_slot_to_drum_rack_pad(track, path, pad_note)
                                {
                                    Ok(slot_idx) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Set drum rack pad {pad_note} to slot {}",
                                            slot_idx + 1
                                        )));
                                    }
                                    Err(e) => {
                                        if preserve_browser_context {
                                            preserve_sample_browser_context_for_loaded_sample(
                                                &mut editor,
                                                "",
                                            );
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error setting drum rack pad: {e}"
                                        )));
                                    }
                                }
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Drum rack pad is missing a track, pad, or sample path"
                                        .to_string(),
                                ));
                            }
                        }
                    }
                    "delete-rack-slot" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let slot_idx = extract_usize_from_payload(&payload, "slot");
                        match (track, slot_idx) {
                            (Some(track), Some(slot_idx)) => {
                                match app.apply_recorded_instrument_binding_mutation(
                                    track,
                                    "Delete rack layer",
                                    |app| app.graph_controller().delete_rack_slot(track, slot_idx),
                                ) {
                                    Ok(()) => {
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Deleted rack layer {}",
                                            slot_idx + 1
                                        )));
                                    }
                                    Err(error) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error deleting rack layer: {error}"
                                        )));
                                    }
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack layer deletion is missing a track or layer".to_string(),
                            )),
                        }
                    }
                    "add-rack-slot-effect" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let name = extract_string_from_payload(&payload, "name");
                        let builtin = extract_bool_from_payload(&payload, "builtin");
                        match (track, rack_slot, name) {
                            (Some(track), Some(rack_slot), Some(name)) => {
                                let is_builtin = builtin
                                    || sequencer::effects::EffectDescriptor::builtin_insert(&name)
                                        .is_some()
                                    || sequencer::effects::conv_reverb::is_dgen_builtin(&name);
                                let result = app.apply_recorded_rack_effect_chain_mutation(
                                    track,
                                    rack_slot,
                                    "Add rack-slot effect",
                                    |app| if is_builtin {
                                        app.add_builtin_rack_slot_effect_sync(track, rack_slot, &name)
                                    } else {
                                        app.add_rack_slot_effect_sync(track, rack_slot, &name)
                                    },
                                );
                                match result {
                                    Ok(_) => {
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error adding rack-slot effect: {error}"),
                                    )),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect drop is incomplete".to_string(),
                            )),
                        }
                    }
                    "insert-rack-slot-effect-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let target_slot = extract_usize_from_payload(&payload, "slot");
                        let name = extract_string_from_payload(&payload, "name");
                        let builtin = extract_bool_from_payload(&payload, "builtin");
                        match (track, rack_slot, target_slot, name) {
                            (Some(track), Some(rack_slot), Some(target_slot), Some(name)) => {
                                let result = app.apply_recorded_rack_effect_chain_mutation(
                                    track,
                                    rack_slot,
                                    "Insert rack-slot effect",
                                    |app| if builtin {
                                        app.insert_builtin_rack_slot_effect_before_slot_sync(
                                            track,
                                            rack_slot,
                                            target_slot,
                                            &name,
                                        )
                                    } else {
                                        app.insert_rack_slot_effect_before_slot_sync(
                                            track,
                                            rack_slot,
                                            target_slot,
                                            &name,
                                        )
                                    },
                                );
                                match result {
                                    Ok(_) => {
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error inserting rack-slot effect: {error}"),
                                    )),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect insert is incomplete".to_string(),
                            )),
                        }
                    }
                    "delete-rack-slot-effect" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
                        match (track, rack_slot, effect_slot) {
                            (Some(track), Some(rack_slot), Some(effect_slot)) => match app
                                .apply_recorded_rack_effect_chain_mutation(
                                    track,
                                    rack_slot,
                                    "Delete rack-slot effect",
                                    |app| app.delete_rack_slot_effect_slot(
                                        track, rack_slot, effect_slot,
                                    ),
                                )
                            {
                                Ok(()) => {
                                    refresh_instrument_panel_reactive(
                                        &mut editor,
                                        &app,
                                        track,
                                        &selected_steps,
                                        &ui_epoch,
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting rack-slot effect: {error}"
                                ))),
                            },
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect deletion is incomplete".to_string(),
                            )),
                        }
                    }
                    "move-rack-slot-effect" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let requested_target = extract_usize_from_payload(&payload, "target-slot");
                        let position = extract_string_from_payload(&payload, "position");
                        match (track, rack_slot, source_slot) {
                            (Some(track), Some(rack_slot), Some(source_slot)) => {
                                let target_slot = if position.as_deref() == Some("append") {
                                    rack_slot_snapshot_for_host(&state, track, rack_slot).and_then(
                                        |slot| {
                                            slot.effect_slots
                                                .iter()
                                                .rposition(|effect| effect.node_id != 0)
                                        },
                                    )
                                } else {
                                    requested_target.map(|target| {
                                        if source_slot < target {
                                            target.saturating_sub(1)
                                        } else {
                                            target
                                        }
                                    })
                                };
                                if let Some(target_slot) = target_slot {
                                    match app.apply_recorded_rack_effect_chain_mutation(
                                        track,
                                        rack_slot,
                                        "Move rack-slot effect",
                                        |app| app.move_rack_slot_effect_slot_sync(
                                            track,
                                            rack_slot,
                                            source_slot,
                                            target_slot,
                                        ),
                                    ) {
                                        Ok(()) => {
                                            refresh_instrument_panel_reactive(
                                                &mut editor,
                                                &app,
                                                track,
                                                &selected_steps,
                                                &ui_epoch,
                                            );
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error moving rack-slot effect: {error}"),
                                        )),
                                    }
                                } else {
                                    editor.handle_host_event(HostEvent::Status(
                                        "Rack-slot FX move is missing a destination".to_string(),
                                    ));
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect move is missing its source".to_string(),
                            )),
                        }
                    }
                    "set-rack-slot-effect-param" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
                        let param = extract_usize_from_payload(&payload, "param");
                        let value = extract_f32_from_payload(&payload, "value");
                        match (track, rack_slot, effect_slot, param, value) {
                            (
                                Some(track),
                                Some(rack_slot),
                                Some(effect_slot),
                                Some(param),
                                Some(value),
                            ) => {
                                let outcome = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotEffectParam {
                                        track,
                                        rack_slot_idx: rack_slot,
                                        effect_slot_idx: effect_slot,
                                        param_idx: param,
                                        value,
                                    },
                                );
                                if outcome.is_ok() {
                                    rack_control_snapshot_dirty = true;
                                    if rack_slot_effect_param_needs_panel_rebuild(
                                        &state,
                                        track,
                                        rack_slot,
                                        effect_slot,
                                        param,
                                    ) {
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                    } else {
                                        refresh_rack_direct_param_reactive(
                                            &mut editor,
                                            &app,
                                            &state,
                                            track,
                                            RackDirectDisplayTarget::EffectParam {
                                                rack_slot,
                                                effect_slot,
                                                param_idx: param,
                                            },
                                            &selected_steps,
                                            false,
                                            &ui_epoch,
                                        );
                                    }
                                } else if let Err(error) = outcome {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error setting rack-slot effect parameter: {error:?}"
                                    )));
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect parameter edit is incomplete".to_string(),
                            )),
                        }
                    }
                    "set-rack-slot-effect-plock" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
                        let param = extract_usize_from_payload(&payload, "param");
                        let value = extract_f32_from_payload(&payload, "value");
                        match (track, rack_slot, effect_slot, param, value) {
                            (
                                Some(track),
                                Some(rack_slot),
                                Some(effect_slot),
                                Some(param),
                                Some(value),
                            ) => {
                                let steps: Vec<usize> =
                                    selected_steps.lock().unwrap().iter().copied().collect();
                                let outcome = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotEffectPlockMulti {
                                        track,
                                        steps,
                                        rack_slot_idx: rack_slot,
                                        effect_slot_idx: effect_slot,
                                        param_idx: param,
                                        value,
                                    },
                                );
                                if !outcome
                                    .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp)
                                {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Rack-slot effect parameter locks were not changed"
                                    )));
                                } else {
                                    rack_control_snapshot_dirty = true;
                                    if rack_slot_effect_param_needs_panel_rebuild(
                                        &state,
                                        track,
                                        rack_slot,
                                        effect_slot,
                                        param,
                                    ) {
                                        sync_rack_slot_instrument_authoring_display(
                                            &mut editor,
                                            &app,
                                            &state,
                                            track,
                                            &selected_steps,
                                        );
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        refresh_rack_direct_param_reactive(
                                            &mut editor,
                                            &app,
                                            &state,
                                            track,
                                            RackDirectDisplayTarget::EffectParam {
                                                rack_slot,
                                                effect_slot,
                                                param_idx: param,
                                            },
                                            &selected_steps,
                                            true,
                                            &ui_epoch,
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect parameter-lock edit is incomplete".to_string(),
                            )),
                        }
                    }
                    "set-rack-slot-effect-param-option" | "set-rack-slot-effect-plock-option" => {
                        let write_plock = name == "set-rack-slot-effect-plock-option";
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
                        let param_idx = extract_usize_from_payload(&payload, "param");
                        let label = extract_string_from_payload(&payload, "label");
                        match (track, rack_slot, effect_slot, param_idx, label) {
                            (
                                Some(track),
                                Some(rack_slot),
                                Some(effect_slot),
                                Some(param_idx),
                                Some(label),
                            ) => {
                                let result = if write_plock {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    app.rack_slot_effect_option_value(
                                        track, rack_slot, effect_slot, param_idx, &label,
                                    )
                                    .and_then(|value| {
                                        let outcome = app::try_apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotEffectPlockMulti {
                                                track,
                                                steps,
                                                rack_slot_idx: rack_slot,
                                                effect_slot_idx: effect_slot,
                                                param_idx,
                                                value,
                                            },
                                        );
                                        outcome
                                            .map_err(|error| format!("{error:?}"))
                                            .and_then(|outcome| {
                                                (outcome != app::edit::EditOutcome::NoOp)
                                                    .then_some(())
                                                    .ok_or_else(|| {
                                                    "Rack-slot effect parameter locks were not changed"
                                                        .to_string()
                                                    })
                                            })
                                    })
                                } else {
                                    app.rack_slot_effect_option_value(
                                        track, rack_slot, effect_slot, param_idx, &label,
                                    )
                                    .and_then(|value| {
                                        app::try_apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotEffectParam {
                                                track,
                                                rack_slot_idx: rack_slot,
                                                effect_slot_idx: effect_slot,
                                                param_idx,
                                                value,
                                            },
                                        )
                                        .map(|_| ())
                                        .map_err(|error| format!("{error:?}"))
                                    })
                                };
                                match result {
                                    Ok(()) => {
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error setting rack-slot effect option: {error}"),
                                    )),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack-slot effect option edit is incomplete".to_string(),
                            )),
                        }
                    }
                    "group-track-to-instrument-rack" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        match track {
                            Some(track) => {
                                match app.group_track_to_instrument_rack_recorded(track) {
                                    Ok(()) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(
                                            "Grouped track to Instrument Rack".to_string(),
                                        ));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Could not group track: {error}"),
                                    )),
                                }
                            }
                            None => editor.handle_host_event(HostEvent::Status(
                                "No track selected for grouping".to_string(),
                            )),
                        }
                    }
                    "add-rack-instrument-slot" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let name = extract_string_from_payload(&payload, "name");
                        match (track, name) {
                            (Some(track), Some(name)) => {
                                match app.add_saved_instrument_slot_to_rack_sync(track, &name) {
                                    Ok(slot_idx) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Added rack instrument layer {}: {}",
                                            slot_idx + 1,
                                            name
                                        )));
                                    }
                                    Err(error) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error adding rack instrument layer: {error}"
                                        )));
                                    }
                                }
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Rack instrument layer is missing a track or instrument name"
                                        .to_string(),
                                ));
                            }
                        }
                    }
                    "replace-rack-slot-instrument" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let name = extract_string_from_payload(&payload, "name");
                        match (track, slot, name) {
                            (Some(track), Some(slot), Some(name)) => {
                                match app.replace_rack_slot_with_saved_instrument_sync(
                                    track, slot, &name,
                                ) {
                                    Ok(()) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Replaced rack layer {} with {}",
                                            slot + 1,
                                            name
                                        )));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error replacing rack instrument layer: {error}"),
                                    )),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Rack instrument replacement is missing a track, slot, or instrument name"
                                    .to_string(),
                            )),
                        }
                    }
                    "add-rack-instrument-pad" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let pad_note = extract_i32_from_payload(&payload, "pad-note");
                        let name = extract_string_from_payload(&payload, "name");
                        match (track, pad_note, name) {
                            (Some(track), Some(pad_note), Some(name)) => {
                                match app.add_saved_instrument_slot_to_drum_rack_pad_sync(
                                    track, pad_note, &name,
                                ) {
                                    Ok(slot_idx) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Set drum rack pad {pad_note} to slot {}: {}",
                                            slot_idx + 1,
                                            name
                                        )));
                                    }
                                    Err(error) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error setting drum rack instrument pad: {error}"
                                        )));
                                    }
                                }
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Drum rack instrument pad is missing a track, pad, or instrument name"
                                        .to_string(),
                                ));
                            }
                        }
                    }
                    "add-track-modulator" => match app.graph_controller().add_modulator_track()
                        .and_then(|idx| {
                            app.commit_created_track(idx, "Add modulator track")?;
                            Ok(idx)
                        }) {
                        Ok(idx) => {
                            sync_after_instrument_track_apply(
                                &mut app,
                                &mut editor,
                                &state,
                                idx,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                            );
                            let new_name = app.tracks[idx].clone();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added modulator track {}: {new_name}",
                                idx + 1
                            )));
                        }
                        Err(e) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error adding modulator track: {e}"
                            )));
                        }
                    },
                    "reanalyze-sample" => {
                        let Some(track) = current_track_for_app(&mut app, &current_track) else {
                            editor.handle_host_event(HostEvent::Status(
                                "No sample loaded on this track".to_string(),
                            ));
                            continue;
                        };
                        let Some(path) = app
                            .sampler_paths
                            .get(track)
                            .and_then(|path| path.as_ref())
                            .cloned()
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "No sample loaded on this track".to_string(),
                            ));
                            continue;
                        };
                        match sequencer::sampler::load_wav_buffer(lg_raw, &path) {
                            Ok(loaded) => {
                                app.submit_sample_analysis(&loaded);
                                let new_buffer_id = loaded.buffer_id;
                                let sample_rate = loaded.sample_rate;
                                app.graph_controller().send_sample_to_all_voices(
                                    track,
                                    new_buffer_id,
                                    sample_rate,
                                );
                                app.graph.track_buffer_ids[track] = new_buffer_id;
                                app.graph.track_sample_rates[track] = sample_rate;
                                let sample_name = app.tracks[track].clone();
                                app.register_loaded_sample_path(
                                    &sample_name,
                                    new_buffer_id,
                                    path.clone(),
                                );
                                app.publish_sampler_analysis_runtime(track);
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "instrument-panel",
                                    build_instrument_panel_value(&app, track, &selected_steps),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.handle_host_event(HostEvent::Status(
                                    "Re-analyzing sample".to_string(),
                                ));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error re-analyzing sample: {error}"
                                )));
                            }
                        }
                    }
                    "set-convolution-reverb-ir" => {
                        let path_str = extract_path_from_payload(&payload);
                        // bus >= 0 means a bus effect; absent/-1 means a track effect.
                        let bus = extract_usize_from_payload(&payload, "bus");
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        match (slot, path_str) {
                            (Some(slot), Some(path_str)) => {
                                let path = Path::new(&path_str);
                                let reference = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(path_str.as_str())
                                    .to_string();
                                let result = if let Some(bus_idx) = bus {
                                    app.apply_recorded_bus_effect_value_mutation(
                                        bus_idx,
                                        slot,
                                        "Set bus convolution IR",
                                        "convolution-ir",
                                        |app| app.set_conv_reverb_ir_bus(
                                            bus_idx,
                                            slot,
                                            path,
                                            &reference,
                                        ),
                                    )
                                } else if let Some(track) = track {
                                    app::edit::apply_recorded_track_effect_ir_mutation(
                                        &mut app,
                                        track,
                                        slot,
                                        path,
                                        &reference,
                                    )
                                    .map(|_| ())
                                    .map_err(|error| format!("{error:?}"))
                                } else {
                                    Err("need a track or bus".to_string())
                                };
                                match result {
                                    Ok(()) => {
                                        // Refresh the relevant effects view so the label updates.
                                        let rt = editor.runtime_mut();
                                        if bus.is_some() {
                                            rt.set_reactive(
                                                "SEQ",
                                                "bus-effects",
                                                build_bus_effects_value_for_selection(
                                                    &app,
                                                    Some(&selected_steps),
                                                ),
                                            );
                                        } else if let Some(track) = track {
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
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Loaded IR: {reference}"
                                        )));
                                    }
                                    Err(e) => editor.handle_host_event(HostEvent::Status(format!(
                                        "Error loading IR: {e}"
                                    ))),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "set-convolution-reverb-ir: need slot, path".to_string(),
                            )),
                        }
                    }
                    "swap-track-builtin-instrument" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let instrument = extract_string_from_payload(&payload, "name");
                        match (track, instrument.as_deref()) {
                            (Some(track), Some("sampler")) => {
                                match load_or_convert_sampler_track(
                                    &mut app,
                                    &mut editor,
                                    &state,
                                    &current_track,
                                    &mut track_names,
                                    &selected_steps,
                                    lg_raw,
                                    track,
                                    None,
                                ) {
                                    Ok(result) => {
                                        let _ = editor
                                            .runtime_mut()
                                            .eval_str("(set! sbrowser-tab \"samples\")");
                                        let status = result.reset_summary.map_or_else(
                                            || format!("Sampler already active ({})", result.name),
                                            |summary| {
                                                host_commands::instrument_swap_status(
                                                    "sampler", summary,
                                                )
                                            },
                                        );
                                        editor.handle_host_event(HostEvent::Status(status));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Cannot convert track to sampler: {error}"),
                                    )),
                                }
                            }
                            (Some(_), Some(name)) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Builtin instrument conversion is not supported for {name}"
                                )))
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Builtin instrument swap is missing a track or name".to_string(),
                            )),
                        }
                    }
                    "load-sample-into-track" | "convert-track-to-sampler" => {
                        let path_str = extract_path_from_payload(&payload);
                        let track = extract_usize_from_payload(&payload, "track");
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        eprintln!(
                            "sample-host-command: load-sample-into-track payload={payload:?}; extracted_path={path_str:?}; extracted_track={track:?}; preserve_browser_context={preserve_browser_context}"
                        );
                        match (track, path_str) {
                            (Some(track), Some(path_str)) => {
                                if preserve_browser_context {
                                    preserve_sample_browser_context_for_loaded_sample(
                                        &mut editor,
                                        &path_str,
                                    );
                                }
                                let path = Path::new(&path_str);
                                match load_or_convert_sampler_track(
                                    &mut app,
                                    &mut editor,
                                    &state,
                                    &current_track,
                                    &mut track_names,
                                    &selected_steps,
                                    lg_raw,
                                    track,
                                    Some(path),
                                ) {
                                    Ok(result) => {
                                        let status = result.reset_summary.map_or_else(
                                            || {
                                                format!(
                                                    "Loaded sample on track {}: {}",
                                                    track + 1,
                                                    result.name
                                                )
                                            },
                                            |summary| {
                                                format!(
                                                    "{}; loaded {}",
                                                    host_commands::instrument_swap_status(
                                                        "sampler", summary,
                                                    ),
                                                    result.name
                                                )
                                            },
                                        );
                                        editor.handle_host_event(HostEvent::Status(status));
                                    }
                                    Err(e) => {
                                        if preserve_browser_context {
                                            preserve_sample_browser_context_for_loaded_sample(
                                                &mut editor,
                                                "",
                                            );
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error loading sample: {e}"
                                        )));
                                    }
                                }
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Sample drop is missing a track or path".to_string(),
                                ));
                            }
                        }
                    }
                    "add-track-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        let group_id = extract_usize_from_payload(&payload, "group-id")
                            .map(|group_id| group_id as u64);
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        eprintln!(
                            "sample-host-command: add-track-sample payload={payload:?}; extracted_path={path_str:?}; preserve_browser_context={preserve_browser_context}"
                        );
                        if let Some(path_str) = path_str {
                            if preserve_browser_context {
                                preserve_sample_browser_context_for_loaded_sample(
                                    &mut editor,
                                    &path_str,
                                );
                            }
                            let path = Path::new(&path_str);
                            let groups_before = app.groups.clone();
                            match app.graph_controller().add_track(path) {
                                Ok(idx) => {
                                    host_commands::add_new_track_to_group(&mut app, idx, group_id);
                                    if let Err(error) = app.commit_created_track(idx, "Add sample track") {
                                        app.groups = groups_before;
                                        *track_groups.lock().unwrap() = app.groups.clone();
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error adding track: {error}"
                                        )));
                                        continue;
                                    }
                                    *track_groups.lock().unwrap() = app.groups.clone();
                                    register_waveform_sample(path);
                                    current_track.store(idx, Ordering::Relaxed);
                                    let new_name = app.tracks[idx].clone();
                                    track_names.push(new_name.clone());
                                    // Update pan IDs for new track
                                    {
                                        let mut pan_ids = track_pan_ids.lock().unwrap();
                                        pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                                        push_solo_mutes(lg_raw, &state, &pan_ids);
                                    }
                                    // Extend record_armed for new track
                                    record_armed.lock().unwrap().push(false);
                                    // Update reactive state
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "num-tracks",
                                        Value::Number(track_names.len() as f64),
                                    );
                                    rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                                    set_current_track_reactive(rt, app.tracks.len(), idx);
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&track_names),
                                    );
                                    sync_all_track_sequencer_state(
                                        rt,
                                        &state,
                                        &app,
                                        idx,
                                        &selected_steps,
                                    );
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, idx));
                                    sync_step_param_lists(rt, &state, idx);
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_groups_bindings(rt, &app.groups);
                                    sync_bus_mixer_state(rt, &app);
                                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            idx,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(&state, idx, &selected_steps),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, idx, &selected_steps),
                                    );
                                    *accumulator_names.lock().unwrap() =
                                        build_accumulator_names(&app);
                                    let selected_neural_snapshot =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    sync_track_params_with_neural_selection(
                                        rt,
                                        &app,
                                        &state,
                                        idx,
                                        &selected_steps,
                                        Some(&selected_neural_snapshot),
                                    );
                                    sync_fx_param_binding_fields_with_neural_selection(
                                        rt,
                                        &app,
                                        &state,
                                        idx,
                                        &selected_steps,
                                        Some(&selected_neural_snapshot),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            idx,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Added track {}: {new_name}",
                                        idx + 1
                                    )));
                                }
                                Err(e) => {
                                    if preserve_browser_context {
                                        preserve_sample_browser_context_for_loaded_sample(
                                            &mut editor,
                                            "",
                                        );
                                    }
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error adding track: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-instrument" | "swap-track-instrument" => {
                        if let Some(pending) = pending_saved_instrument_load.as_ref() {
                            let escaped = escape_lisp_string(&pending.name);
                            let _ = editor.runtime_mut().eval_str(&format!(
                                "(set! sbrowser-loading-instrument-name \"{escaped}\")"
                            ));
                            editor.handle_host_event(HostEvent::Status(
                                "An instrument is already loading".to_string(),
                            ));
                            continue;
                        }
                        let Some(instrument_name) = extract_string_from_payload(&payload, "name")
                        else {
                            let _ = editor
                                .runtime_mut()
                                .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                            editor.handle_host_event(HostEvent::Status(
                                "Instrument load is missing a name".to_string(),
                            ));
                            continue;
                        };
                        let target = if name == "swap-track-instrument" {
                            let Some(track) = extract_usize_from_payload(&payload, "track") else {
                                let _ = editor
                                    .runtime_mut()
                                    .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                                editor.handle_host_event(HostEvent::Status(
                                    "Instrument swap is missing a track".to_string(),
                                ));
                                continue;
                            };
                            match capture_instrument_swap_target(&app, track) {
                                Ok(target) => target,
                                Err(error) => {
                                    let _ = editor
                                        .runtime_mut()
                                        .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Cannot swap instrument: {error}"
                                    )));
                                    continue;
                                }
                            }
                        } else {
                            SavedInstrumentLoadTarget::AddTrack {
                                group_id: extract_usize_from_payload(&payload, "group-id")
                                    .map(|group_id| group_id as u64),
                            }
                        };
                        let escaped = escape_lisp_string(&instrument_name);
                        let _ = editor.runtime_mut().eval_str(&format!(
                            "(set! sbrowser-loading-instrument-name \"{escaped}\")"
                        ));
                        let source =
                            match sequencer::lisp_host::load_instrument_source(&instrument_name) {
                                Ok(source) => source,
                                Err(error) => {
                                    let _ = editor
                                        .runtime_mut()
                                        .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error loading instrument source: {error}"
                                    )));
                                    continue;
                                }
                            };
                        let run_mode = match sequencer::lisp_host::load_instrument_run_mode(
                            &instrument_name,
                        ) {
                            Ok(run_mode) => run_mode,
                            Err(error) => {
                                let _ = editor
                                    .runtime_mut()
                                    .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error loading instrument metadata: {error}"
                                )));
                                continue;
                            }
                        };
                        if let Some(cached_result) = try_apply_cached_saved_instrument(
                            &mut app,
                            target,
                            &instrument_name,
                            &source,
                            run_mode,
                        ) {
                            let _ = editor
                                .runtime_mut()
                                .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                            match cached_result {
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
                                            cached_track_peak_levels: &cached_track_peak_levels,
                                            group_id,
                                            track_groups: &track_groups,
                                            ui_epoch: &ui_epoch,
                                            lg_raw,
                                        },
                                    )
                                }
                                Ok(SavedInstrumentLoadApply::Swapped { summary }) => {
                                    finish_swapped_instrument_track(
                                        &instrument_name,
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
                                    )
                                }
                                Err(error) => {
                                    let action = match target {
                                        SavedInstrumentLoadTarget::AddTrack { .. } => {
                                            "adding instrument track"
                                        }
                                        SavedInstrumentLoadTarget::SwapTrack { .. } => {
                                            "swapping instrument"
                                        }
                                    };
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error {action}: {error}"
                                    )))
                                }
                            }
                            continue;
                        }
                        let sample_rate = app.graph.sample_rate;
                        let asset_base =
                            sequencer::lisp_host::instrument_source_path(&instrument_name)
                                .ok()
                                .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
                        let compile_source = source.clone();
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result =
                                sequencer::lisp_host::compile_and_load_instrument_with_asset_base(
                                    &compile_source,
                                    sample_rate,
                                    asset_base.as_deref(),
                                );
                            let _ = tx.send(result);
                        });
                        pending_saved_instrument_load = Some(PendingSavedInstrumentLoad {
                            name: instrument_name.clone(),
                            source,
                            run_mode,
                            target,
                            receiver: rx,
                        });
                        let action = match target {
                            SavedInstrumentLoadTarget::AddTrack { .. } => "Loading instrument",
                            SavedInstrumentLoadTarget::SwapTrack { .. } => {
                                "Loading instrument for swap"
                            }
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "{action}: {instrument_name}"
                        )));
                        editor.mark_needs_redraw();
                    }
                    "move-saved-instrument" => {
                        let name = extract_string_from_payload(&payload, "name");
                        let folder = extract_string_from_payload(&payload, "folder");
                        match (name, folder) {
                            (Some(name), Some(folder)) => {
                                match sequencer::lisp_host::move_saved_instrument(&name, &folder) {
                                    Ok(new_name) => {
                                        if let Err(error) = editor
                                            .runtime_mut()
                                            .eval_str("(sbrowser-refresh-buffer)")
                                        {
                                            eprintln!(
                                                "instrument browser: failed to refresh after move: {error:?}"
                                            );
                                        }
                                        editor.refresh_runtime_side_effects();
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Moved instrument: {new_name}"
                                        )));
                                        editor.mark_needs_redraw();
                                    }
                                    Err(error) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error moving instrument: {error}"
                                        )));
                                    }
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Instrument move is missing a name or folder".to_string(),
                            )),
                        }
                    }
                    "delete-track" => {
                        let track = match &payload {
                            Value::Map(map) => {
                                map.get("track").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        }
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                        let request_id = if state.is_playing() {
                            let request_id = state.request_track_delete_boundary(track);
                            let wait_deadline = Instant::now() + Duration::from_millis(250);
                            while !state.topology_edit_ready(request_id)
                                && Instant::now() < wait_deadline
                            {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            if !state.topology_edit_ready(request_id) {
                                state.complete_topology_edit(request_id);
                                state.publish_scheduler_snapshot();
                                editor.handle_host_event(HostEvent::Status(
                                    "Delete timed out waiting for playback boundary".to_string(),
                                ));
                                continue;
                            }
                            Some(request_id)
                        } else {
                            None
                        };

                        match app.delete_track_recorded(track) {
                            Ok(new_idx) => {
                                if let Some(request_id) = request_id {
                                    state.complete_topology_edit(request_id);
                                    state.publish_scheduler_snapshot();
                                }
                                current_track.store(new_idx, Ordering::Relaxed);
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
                                cached_track_peak_levels = read_track_peak_levels(
                                    app.graph.lg,
                                    &track_pan_ids.lock().unwrap(),
                                );
                                cached_bus_peak_levels =
                                    read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                                (cached_modulator_phases, cached_modulator_levels) =
                                    read_modulator_display_values(app.graph.lg, &app);
                                last_meter_poll_at = Instant::now();
                                *record_armed.lock().unwrap() = app.graph.record_armed.clone();
                                *track_groups.lock().unwrap() = app.groups.clone();

                                let rt = editor.runtime_mut();
                                sync_track_topology_state(
                                    rt,
                                    &app,
                                    &state,
                                    &mut track_names,
                                    new_idx,
                                    &selected_steps,
                                    &piano_roll_selection,
                                    &accumulator_names,
                                    &record_armed,
                                    &cached_track_peak_levels,
                                );
                                sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                                sync_modulator_phase_fields(rt, &cached_modulator_phases);
                                sync_modulator_level_fields(rt, &cached_modulator_levels);
                                rt.clear_subtree_effects_for_named_target("*sequencer*");
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                refresh_visible_track_topology_layouts(&mut editor);
                                prev_track_playheads = track_playheads_snapshot(&state, &app);
                                prev_track_button_states = track_button_state_snapshot(&state);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Deleted track {}",
                                    track + 1
                                )));
                            }
                            Err(e) => {
                                if let Some(request_id) = request_id {
                                    state.complete_topology_edit(request_id);
                                    state.publish_scheduler_snapshot();
                                }
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting track: {e}"
                                )));
                            }
                        }
                    }
                    "load-instrument-preset" => {
                        if let Value::Map(ref map) = payload {
                            let preset_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(name) => Some(name.clone()),
                                    _ => None,
                                });
                            if let Some(preset_name) = preset_name {
                                let track = current_track.load(Ordering::Relaxed);
                                let is_rack = app.graph.track_instrument_types.get(track)
                                    == Some(&sequencer::sequencer::InstrumentType::Rack);
                                let load_result = if is_rack {
                                    app.load_rack_preset_onto_track(track, &preset_name)
                                } else {
                                    load_instrument_preset_into_track(&mut app, track, &preset_name)
                                };
                                match load_result {
                                    Ok(()) => {
                                        if is_rack {
                                            sync_after_instrument_track_apply(
                                                &mut app,
                                                &mut editor,
                                                &state,
                                                track,
                                                &current_track,
                                                &mut track_names,
                                                &track_pan_ids,
                                                &record_armed,
                                                &selected_steps,
                                                &accumulator_names,
                                                &cached_track_peak_levels,
                                                &cached_bus_peak_levels,
                                                &ui_epoch,
                                                lg_raw,
                                            );
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        } else {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "instrument-panel",
                                                build_instrument_panel_value(
                                                    &app,
                                                    track,
                                                    &selected_steps,
                                                ),
                                            );
                                            sync_sidebar_browser(rt, &app, track);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Loaded preset '{preset_name}'"
                                        )));
                                    }
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error loading preset: {e}"
                                        )));
                                    }
                                }
                            }
                        }
                    }
                    "save-preset" => {
                        if let Value::Map(ref map) = payload {
                            let preset_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                });
                            let overwrite = map
                                .get("overwrite")
                                .map(|cell| match &*cell.borrow() {
                                    Value::Bool(b) => *b,
                                    _ => false,
                                })
                                .unwrap_or(false);
                            if let Some(name) = preset_name {
                                let name = name.trim().to_string();
                                if name.is_empty() {
                                    editor.handle_host_event(HostEvent::Status(
                                        "Preset name cannot be empty".to_string(),
                                    ));
                                } else {
                                    let track = current_track.load(Ordering::Relaxed);
                                    app.ui.cursor_track = track;
                                    let save_result =
                                        app.save_current_track_as_preset(&name, overwrite);
                                    // Refresh sidebar presets list
                                    let rt = editor.runtime_mut();
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    match save_result {
                                        Ok(()) => editor.handle_host_event(HostEvent::Status(
                                            format!("Saved preset '{name}'"),
                                        )),
                                        Err(error) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error saving preset: {error}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "overwrite-preset" => {
                        let track = current_track.load(Ordering::Relaxed);
                        app.ui.cursor_track = track;
                        app.overwrite_loaded_preset();
                        let rt = editor.runtime_mut();
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                    "new-project" => {
                        app.start_new_project();
                        if let Err(error) = clear_project_script_tabs(&mut editor) {
                            editor.handle_host_event(HostEvent::Status(error));
                        }
                        push_project_scratch_to_named_buffer(&mut editor, &app);
                        if let Err(error) =
                            evaluate_project_scratch_on_ui_runtime(&mut editor, &app)
                        {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scratch UI eval error: {error}"
                            )));
                        }
                        selected_steps.lock().unwrap().clear();
                        piano_roll_selection.lock().unwrap().clear();
                        track_names = app.tracks.clone();
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        current_track.store(0, Ordering::Relaxed);
                        {
                            let mut pan_ids = track_pan_ids.lock().unwrap();
                            pan_ids.clear();
                            push_solo_mutes(lg_raw, &state, &pan_ids);
                        }
                        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                        *record_armed.lock().unwrap() = Vec::new();
                        // Keep the shared bus mirror in sync so pull_shared_bus_state
                        // can't restore the previous project's buses.
                        *bus_state.lock().unwrap() = app.buses.clone();
                        // Clear group state so the new project starts ungrouped and
                        // the frame diff doesn't restore the previous project's groups.
                        *track_groups.lock().unwrap() = app.groups.clone();
                        selected_tracks.lock().unwrap().clear();
                        *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                        cached_track_peak_levels.clear();
                        cached_bus_peak_levels =
                            read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                        (cached_modulator_phases, cached_modulator_levels) =
                            read_modulator_display_values(app.graph.lg, &app);
                        last_meter_poll_at = Instant::now();

                        let bpm = state.transport.bpm.load(Ordering::Relaxed);
                        let playing = state.transport.playing.load(Ordering::Relaxed);
                        let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
                        let rt = editor.runtime_mut();
                        sync_pattern_state(rt, &state);
                        sync_project_state(rt, &app);
                        rt.set_reactive("SEQ", "playing", Value::Bool(playing));
                        rt.set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                        rt.set_reactive(
                            "SEQ",
                            "transport-playhead",
                            Value::Number(transport_playhead as f64),
                        );
                        sync_bus_mixer_state(rt, &app);
                        sync_groups_bindings(rt, &app.groups);
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        sync_modulator_phase_fields(rt, &cached_modulator_phases);
                        sync_modulator_level_fields(rt, &cached_modulator_levels);
                        rt.set_reactive("SEQ", "num-tracks", Value::Number(0.0));
                        set_current_track_reactive(rt, 0, 0);
                        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-names", Value::List(vec![]));
                        rt.set_reactive("SEQ", "record-armed", Value::List(vec![]));
                        rt.set_reactive("SEQ", "selected-steps", Value::List(vec![]));
                        sync_playhead_fields(rt, 0, 1);
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
                        sync_sidebar_browser(rt, &app, 0);
                        rt.clear_subtree_effects_for_named_target("*sequencer*");
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        refresh_visible_track_topology_layouts(&mut editor);

                        prev_current_track = 0;
                        prev_playhead = 0;
                        prev_transport_playhead = transport_playhead;
                        prev_bpm = bpm;
                        prev_playing = playing;
                        prev_pattern_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        prev_track_peak_levels.clear();
                        prev_modulator_phases = cached_modulator_phases.clone();
                        prev_modulator_levels = cached_modulator_levels.clone();
                        prev_bus_playheads = bus_playhead_snapshot(&app);
                        prev_track_playheads = track_playheads_snapshot(&state, &app);
                        prev_track_button_states = track_button_state_snapshot(&state);
                        prev_ui_epoch = ui_epoch.fetch_add(1, Ordering::Relaxed) + 1;

                        editor.handle_host_event(HostEvent::Status("New project".to_string()));
                    }
                    "save-project" => {
                        let _ = current_track_for_app(&mut app, &current_track);
                        pull_named_scratch_buffer_into_project(&editor, &mut app);
                        let requested_name = if let Value::Map(ref map) = payload {
                            map.get("name").and_then(|cell| match &*cell.borrow() {
                                Value::String(name) => Some(name.clone()),
                                _ => None,
                            })
                        } else {
                            None
                        };
                        match app.save_project_with_name(requested_name.as_deref()) {
                            Ok(save_name) => {
                                let rt = editor.runtime_mut();
                                sync_project_state(rt, &app);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Saved project '{save_name}'"
                                )));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error saving project: {error}"
                                )));
                            }
                        }
                    }
                    "load-sound-onto-track" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let path = extract_path_from_payload(&payload);
                        match (track, path) {
                            (Some(track), Some(path)) => {
                                match app.load_sound_onto_track(track, Path::new(&path)) {
                                    Ok(()) => {
                                        sync_after_instrument_track_apply(
                                            &mut app,
                                            &mut editor,
                                            &state,
                                            track,
                                            &current_track,
                                            &mut track_names,
                                            &track_pan_ids,
                                            &record_armed,
                                            &selected_steps,
                                            &accumulator_names,
                                            &cached_track_peak_levels,
                                            &cached_bus_peak_levels,
                                            &ui_epoch,
                                            lg_raw,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(
                                            "Loaded Sound".to_string(),
                                        ));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error loading Sound: {error}"),
                                    )),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Loading a Sound requires a track and path".to_string(),
                            )),
                        }
                    }
                    "promote-preset-to-sound" => {
                        let track = extract_usize_from_payload(&payload, "track")
                            .or_else(|| current_track_for_app(&mut app, &current_track));
                        let preset_name = extract_string_from_payload(&payload, "name");
                        match (track, preset_name) {
                            (Some(track), Some(name)) if !name.trim().is_empty() => {
                                app.ui.cursor_track = track;
                                match app.promote_preset_to_sound(track, &name) {
                                    Ok(_) => {
                                        let rt = editor.runtime_mut();
                                        sync_project_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Added preset '{name}' to Sounds"
                                        )));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error adding preset to Sounds: {error}"),
                                    )),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Preset promotion requires a track and preset name".to_string(),
                            )),
                        }
                    }
                    "add-track-from-sound" => {
                        let path = extract_path_from_payload(&payload);
                        match path {
                            Some(path) => match app.add_track_from_sound(Path::new(&path))
                                .and_then(|track| {
                                    app.commit_created_track(track, "Add Sound track")?;
                                    Ok(track)
                                }) {
                                Ok(track) => {
                                    sync_after_instrument_track_apply(
                                        &mut app,
                                        &mut editor,
                                        &state,
                                        track,
                                        &current_track,
                                        &mut track_names,
                                        &track_pan_ids,
                                        &record_armed,
                                        &selected_steps,
                                        &accumulator_names,
                                        &cached_track_peak_levels,
                                        &cached_bus_peak_levels,
                                        &ui_epoch,
                                        lg_raw,
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(
                                        "Added track from Sound".to_string(),
                                    ));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding Sound track: {error}"
                                ))),
                            },
                            None => editor.handle_host_event(HostEvent::Status(
                                "Sound drop is missing a path".to_string(),
                            )),
                        }
                    }
                    "load-project" => {
                        let requested_name = if let Value::Map(ref map) = payload {
                            map.get("name").and_then(|cell| match &*cell.borrow() {
                                Value::String(name) => Some(name.clone()),
                                _ => None,
                            })
                        } else {
                            None
                        };
                        let Some(project_name) =
                            requested_name.filter(|name| !name.trim().is_empty())
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Error loading project: missing project name".to_string(),
                            ));
                            continue;
                        };
                        eprintln!("metal_seq: host load-project name={project_name}");
                        ui_invalidations.clear();
                        expanded_step_projection.clear();
                        match app.queue_project_load_named(&project_name) {
                            Ok(()) => {
                                eprintln!("metal_seq: queued project load name={project_name}");
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Opening project '{project_name}'..."
                                )));
                            }
                            Err(error) => {
                                eprintln!(
                                    "metal_seq: queue project load failed name={} error={}",
                                    project_name, error
                                );
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error loading project: {error}"
                                )));
                            }
                        }
                    }
                    "remove-project-script-from-scratch" => {
                        let Some(source_path) = extract_string_from_payload(&payload, "path")
                            .filter(|path| !path.trim().is_empty())
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Error removing project script: missing path".to_string(),
                            ));
                            continue;
                        };
                        remove_project_script_from_scratch(&mut editor, &source_path);
                    }
                    "open-script-source-tab" => {
                        let Some(path_str) = extract_string_from_payload(&payload, "path")
                            .filter(|path| !path.trim().is_empty())
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Error opening script source: missing path".to_string(),
                            ));
                            continue;
                        };
                        let label = extract_string_from_payload(&payload, "label")
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or_else(|| {
                                Path::new(&path_str)
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("Script")
                                    .to_string()
                            });
                        let read_only = extract_bool_from_payload(&payload, "read-only");
                        match register_script_source_tab(
                            &mut editor,
                            Path::new(&path_str),
                            &label,
                            &path_str,
                        ) {
                            Ok(buffer_name) => {
                                if read_only {
                                    if let Some(buffer) = editor
                                        .buffers
                                        .iter_mut()
                                        .find(|buffer| buffer.name == buffer_name)
                                    {
                                        buffer.read_only = true;
                                    }
                                }
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Opened script source: {label}"
                                )));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error opening script source: {error}"
                                )));
                            }
                        }
                    }
                    "new-script" => {
                        if script_draft_session.is_some() {
                            editor.handle_host_event(HostEvent::Status(
                                "Finish the current script draft before creating another"
                                    .to_string(),
                            ));
                            continue;
                        }
                        let path = match create_new_script_draft_path() {
                            Ok(path) => path,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(error));
                                continue;
                            }
                        };
                        if let Err(error) = std::fs::write(&path, NEW_SCRIPT_TEMPLATE) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Failed to write starter script: {error}"
                            )));
                            continue;
                        }
                        match register_script_source_tab(
                            &mut editor,
                            &path,
                            NEW_SCRIPT_TAB_LABEL,
                            "",
                        ) {
                            Ok(buffer_name) => {
                                script_draft_session = Some(ScriptDraftSession {
                                    temp_path: path,
                                    buffer_name,
                                });
                                let rt = editor.runtime_mut();
                                let _ = rt.eval_str(
                                    r#"
                                    (set! sbrowser-script-save-mode "new-script")
                                    (set! sbrowser-script-name "")
                                    (set! sbrowser-tab "scripts")
                                    "#,
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                let _ = refresh_sample_browser_buffer(&mut editor);
                                editor.handle_host_event(HostEvent::Status(
                                    "Created script draft".to_string(),
                                ));
                            }
                            Err(error) => {
                                let _ = std::fs::remove_file(&path);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error creating script draft: {error}"
                                )));
                            }
                        }
                    }
                    "save-new-script" => {
                        let requested_name =
                            extract_string_from_payload(&payload, "name").unwrap_or_default();
                        let Some(filename) = script_file_name_from_input(&requested_name) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Enter a script name".to_string(),
                            ));
                            continue;
                        };
                        let Some(session) = script_draft_session.clone() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No script draft is active".to_string(),
                            ));
                            continue;
                        };
                        let root = script_root_dir();
                        if let Err(error) = std::fs::create_dir_all(&root) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Failed to create script directory '{}': {error}",
                                root.display()
                            )));
                            continue;
                        }
                        let target = root.join(&filename);
                        if target.exists() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Script already exists: {filename}"
                            )));
                            continue;
                        }
                        let Some(buffer_idx) = editor
                            .buffers
                            .iter()
                            .position(|buffer| buffer.name == session.buffer_name)
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Script draft buffer is no longer open".to_string(),
                            ));
                            continue;
                        };
                        let display_label = filename.trim_end_matches(".lisp").to_string();
                        let mut source = editor.buffers[buffer_idx].text();
                        if source.trim() == NEW_SCRIPT_TEMPLATE.trim() {
                            let escaped_label = escape_lisp_string(&display_label);
                            source = format!(
                                "; ESeqLisp script\n; Source-only scripts can still appear as sequencer tabs.\n(seq-register-script-source-tab \"{escaped_label}\")\n\n"
                            );
                            editor.buffers[buffer_idx].set_text(&source);
                        }
                        let tmp_path = target.with_file_name(format!(
                            ".{}.tmp",
                            target
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("script.lisp")
                        ));
                        if let Err(error) = std::fs::write(&tmp_path, &source).and_then(|_| {
                            std::fs::rename(&tmp_path, &target).or_else(|error| {
                                let _ = std::fs::remove_file(&tmp_path);
                                Err(error)
                            })
                        }) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Failed to save script: {error}"
                            )));
                            continue;
                        }
                        editor.buffers[buffer_idx].path = Some(target.clone());
                        editor.buffers[buffer_idx].dirty = false;
                        if let Some(parent) = session.temp_path.parent() {
                            let _ = std::fs::remove_dir_all(parent);
                        }

                        let target_str = target.to_string_lossy().replace('\\', "/");
                        let load_form = format!(
                            "(seq-script-load-file \"{}\")",
                            escape_lisp_string(&target_str)
                        );
                        if let Err(error) = editor.runtime_mut().eval_str(&load_form) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Saved script but failed to load it: {error:?}"
                            )));
                        }
                        if let Err(error) = register_script_source_tab(
                            &mut editor,
                            &target,
                            &display_label,
                            &target_str,
                        ) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Saved script but failed to register source tab: {error}"
                            )));
                        }
                        script_draft_session = None;
                        let rt = editor.runtime_mut();
                        let _ = rt.eval_str(
                            r#"
                            (set! sbrowser-script-save-mode "")
                            (set! sbrowser-script-name "")
                            (set! sbrowser-tab "scripts")
                            "#,
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        let _ = refresh_sample_browser_buffer(&mut editor);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Saved script: {display_label}"
                        )));
                    }
                    "cancel-new-script" => {
                        if let Some(session) = script_draft_session.take() {
                            let unregister = format!(
                                "(seq-unregister-step-sequencer-tab \"{}\")",
                                escape_lisp_string(&session.buffer_name)
                            );
                            let _ = editor.runtime_mut().eval_str(&unregister);
                            editor.refresh_runtime_side_effects();
                            editor.remove_buffer_by_name(&session.buffer_name);
                            if let Some(parent) = session.temp_path.parent() {
                                let _ = std::fs::remove_dir_all(parent);
                            }
                        }
                        let rt = editor.runtime_mut();
                        let _ = rt.eval_str(
                            r#"
                            (set! sbrowser-script-save-mode "")
                            (set! sbrowser-script-name "")
                            (set! sbrowser-tab "scripts")
                            "#,
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        let _ = refresh_sample_browser_buffer(&mut editor);
                        editor.handle_host_event(HostEvent::Status(
                            "Cancelled script draft".to_string(),
                        ));
                    }
                    "select-rack-slot" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx)) =
                                (map_usize(map, "track"), map_usize(map, "slot"))
                            {
                                let rack = {
                                    app.state
                                        .pattern
                                        .rack_tracks
                                        .lock()
                                        .unwrap()
                                        .get(track)
                                        .cloned()
                                        .flatten()
                                };
                                let selected = rack.is_some_and(|rack| {
                                    app.select_rack_slot(track, &rack, slot_idx)
                                });
                                if selected {
                                    refresh_instrument_panel_reactive(
                                        &mut editor,
                                        &app,
                                        track,
                                        &selected_steps,
                                        &ui_epoch,
                                    );
                                }
                            }
                        }
                    }
                    "select-rack-pad" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(pad_note)) =
                                (map_usize(map, "track"), map_number(map, "pad-note"))
                            {
                                app.set_rack_selected_pad_note(track, pad_note as i32);
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "select-rack-pad-bank" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(bank_start)) =
                                (map_usize(map, "track"), map_number(map, "bank-start"))
                            {
                                app.set_rack_pad_bank_start(track, bank_start as i32);
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-gain" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx), Some(value)) = (
                                map_usize(map, "track"),
                                map_usize(map, "slot"),
                                map_number(map, "value").map(|value| value as f32),
                            ) {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotGain {
                                        track,
                                        slot_idx,
                                        value,
                                    },
                                );
                                rack_control_snapshot_dirty = true;
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam {
                                        slot_idx,
                                        param: RackSlotParam::Gain,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-pan" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx), Some(value)) = (
                                map_usize(map, "track"),
                                map_usize(map, "slot"),
                                map_number(map, "value").map(|value| value as f32),
                            ) {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotPan {
                                        track,
                                        slot_idx,
                                        value,
                                    },
                                );
                                rack_control_snapshot_dirty = true;
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam {
                                        slot_idx,
                                        param: RackSlotParam::Pan,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-mute" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx)) =
                                (map_usize(map, "track"), map_usize(map, "slot"))
                            {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotMute {
                                        track,
                                        slot_idx,
                                        value: map_bool(map, "value"),
                                    },
                                );
                                // Without republishing the scheduler snapshot,
                                // per-trigger panner pushes clobber the new
                                // mute with the stale snapshot's value.
                                rack_control_snapshot_dirty = true;
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam {
                                        slot_idx,
                                        param: RackSlotParam::Mute,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                                // The rack panel's pad/slot dicts carry mute as a
                                // plain value, so rebuild them or the panel shows
                                // stale M/S state.
                                sync_rack_slot_instrument_authoring_display(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    &selected_steps,
                                );
                            }
                        }
                    }
                    "set-rack-slot-solo" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx)) =
                                (map_usize(map, "track"), map_usize(map, "slot"))
                            {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotSolo {
                                        track,
                                        slot_idx,
                                        value: map_bool(map, "value"),
                                    },
                                );
                                rack_control_snapshot_dirty = true;
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam {
                                        slot_idx,
                                        param: RackSlotParam::Solo,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                                sync_rack_slot_instrument_authoring_display(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    &selected_steps,
                                );
                            }
                        }
                    }
                    "set-rack-slot-max-polyphony" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx), Some(value)) = (
                                map_usize(map, "track"),
                                map_usize(map, "slot"),
                                map_usize(map, "value"),
                            ) {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotMaxPolyphony {
                                        track,
                                        slot_idx,
                                        value,
                                    },
                                );
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam {
                                        slot_idx,
                                        param: RackSlotParam::MaxPolyphony,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-choke-group" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx), Some(value)) = (
                                map_usize(map, "track"),
                                map_usize(map, "slot"),
                                map_number(map, "value")
                                    .map(|value| value.round().clamp(0.0, u8::MAX as f64) as u8),
                            ) {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotChokeGroup {
                                        track,
                                        slot_idx,
                                        value,
                                    },
                                );
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam {
                                        slot_idx,
                                        param: RackSlotParam::BaseNote,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-base-note" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(slot_idx), Some(value)) = (
                                map_usize(map, "track"),
                                map_usize(map, "slot"),
                                map_number(map, "value").map(|value| value as f32),
                            ) {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotBaseNoteOffset {
                                        track,
                                        slot_idx,
                                        value,
                                    },
                                );
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-param-plock" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param = map_string(map, "param")
                                .and_then(|name| RackSlotParam::from_name(&name));
                            let value = map_number_or_bool(map, "value").map(|value| value as f32);
                            if let (Some(track), Some(slot_idx), Some(param), Some(value)) =
                                (track, slot_idx, param, value)
                            {
                                let steps: Vec<usize> =
                                    selected_steps.lock().unwrap().iter().copied().collect();
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotParamPlockMulti {
                                        track,
                                        slot_idx,
                                        steps,
                                        param,
                                        value,
                                    },
                                );
                                rack_control_snapshot_dirty = true;
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::SlotParam { slot_idx, param },
                                    &selected_steps,
                                    true,
                                    &ui_epoch,
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "set-rack-macro-value" => {
                        if let Value::Map(ref map) = payload {
                            apply_rack_macro_host_command(
                                &name,
                                map,
                                &mut editor,
                                &mut app,
                                &state,
                                &selected_steps,
                                &ui_epoch,
                                &fx_epoch,
                            );
                        }
                    }
                    "rename-rack-macro" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(id), Some(name)) = (
                                map_usize(map, "track"),
                                map_usize(map, "id"),
                                map_string(map, "name"),
                            ) {
                                if let Some(id) = sequencer::sequencer::RackMacroId::from_index(id)
                                {
                                    app.rename_rack_macro(track, id, name);
                                    refresh_instrument_panel_reactive(
                                        &mut editor,
                                        &app,
                                        track,
                                        &selected_steps,
                                        &ui_epoch,
                                    );
                                }
                            }
                        }
                    }
                    "set-rack-macro-plock" => {
                        if let Value::Map(ref map) = payload {
                            apply_rack_macro_host_command(
                                &name,
                                map,
                                &mut editor,
                                &mut app,
                                &state,
                                &selected_steps,
                                &ui_epoch,
                                &fx_epoch,
                            );
                        }
                    }
                    "map-rack-macro-param" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let id = map_usize(map, "id")
                                .and_then(sequencer::sequencer::RackMacroId::from_index);
                            let kind = map_string(map, "kind");
                            let rack_slot = map_usize(map, "rack-slot");
                            let param = map_string(map, "param");
                            let param_index = map_usize(map, "param-idx");
                            let min = map_number(map, "min").map(|value| value as f32);
                            let max = map_number(map, "max").map(|value| value as f32);
                            if let (
                                Some(track),
                                Some(id),
                                Some(kind),
                                Some(rack_slot),
                                Some(param),
                                Some(param_index),
                                Some(min),
                                Some(max),
                            ) = (track, id, kind, rack_slot, param, param_index, min, max)
                            {
                                let resolved = if kind == "rack-slot-instrument" {
                                    rack_slot_snapshot_for_host(&state, track, rack_slot)
                                        .and_then(|slot| app.rack_slot_instrument_descriptor(&slot))
                                        .and_then(|descriptor| descriptor.params.get(param_index).cloned())
                                        .map(|descriptor| (sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                                            slot: rack_slot, param, param_index,
                                        }, descriptor.user_input_to_stored(min), descriptor.user_input_to_stored(max)))
                                } else if kind == "rack-slot-effect" {
                                    map_usize(map, "effect-slot").and_then(|effect_slot| {
                                        let descriptor = rack_slot_snapshot_for_host(&state, track, rack_slot)?
                                            .effect_descriptors.get(effect_slot)?.params.get(param_index)?.clone();
                                        Some((sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                                            slot: rack_slot, effect_slot, param, param_index,
                                        }, descriptor.user_input_to_stored(min), descriptor.user_input_to_stored(max)))
                                    })
                                } else {
                                    None
                                };
                                if let Some((target, range_min, range_max)) = resolved {
                                    let mapping = sequencer::sequencer::RackMacroMapping {
                                        target,
                                        range_min,
                                        range_max,
                                        curve: sequencer::sequencer::RackMacroCurve::Linear,
                                    };
                                    if let Err(error) = app.map_rack_macro(track, id, mapping) {
                                        eprintln!("rack macro mapping failed: {error}");
                                    } else {
                                        app.set_rack_macro_value(track, id, 0.0);
                                    }
                                    refresh_instrument_panel_reactive(
                                        &mut editor,
                                        &app,
                                        track,
                                        &selected_steps,
                                        &ui_epoch,
                                    );
                                }
                            }
                        }
                    }
                    "unmap-rack-macro-param" => {
                        if let Value::Map(ref map) = payload {
                            if let (Some(track), Some(id), Some(mapping_idx)) = (
                                map_usize(map, "track"),
                                map_usize(map, "id")
                                    .and_then(sequencer::sequencer::RackMacroId::from_index),
                                map_usize(map, "mapping-idx"),
                            ) {
                                app.unmap_rack_macro(track, id, mapping_idx);
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-macro-range" => {
                        if let Value::Map(ref map) = payload {
                            if let (
                                Some(track),
                                Some(id),
                                Some(mapping_idx),
                                Some(range_min),
                                Some(range_max),
                            ) = (
                                map_usize(map, "track"),
                                map_usize(map, "id")
                                    .and_then(sequencer::sequencer::RackMacroId::from_index),
                                map_usize(map, "mapping-idx"),
                                map_number(map, "min").map(|value| value as f32),
                                map_number(map, "max").map(|value| value as f32),
                            ) {
                                app.set_rack_macro_mapping_range(
                                    track,
                                    id,
                                    mapping_idx,
                                    range_min,
                                    range_max,
                                );
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-macro-curve" => {
                        if let Value::Map(ref map) = payload {
                            let curve =
                                map_string(map, "curve").and_then(|curve| match curve.as_str() {
                                    "linear" => Some(sequencer::sequencer::RackMacroCurve::Linear),
                                    "exp" => Some(sequencer::sequencer::RackMacroCurve::Exp),
                                    "log" => Some(sequencer::sequencer::RackMacroCurve::Log),
                                    _ => None,
                                });
                            if let (Some(track), Some(id), Some(mapping_idx), Some(curve)) = (
                                map_usize(map, "track"),
                                map_usize(map, "id")
                                    .and_then(sequencer::sequencer::RackMacroId::from_index),
                                map_usize(map, "mapping-idx"),
                                curve,
                            ) {
                                app.set_rack_macro_mapping_curve(track, id, mapping_idx, curve);
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                    "set-rack-slot-instrument-param-batch"
                    | "set-rack-slot-instrument-plock-batch" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let updates = map_param_updates(map);
                            if let (Some(track), Some(slot_idx), Some(updates)) =
                                (track, slot_idx, updates)
                            {
                                let steps = map_usize_list(map, "steps").unwrap_or_else(|| {
                                    selected_steps
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .copied()
                                        .collect::<Vec<_>>()
                                });
                                let commands = rack_slot_snapshot_for_host(&state, track, slot_idx)
                                    .and_then(|slot| app.rack_slot_instrument_descriptor(&slot))
                                    .map(|descriptor| {
                                        updates.into_iter().filter_map(|(param_idx, user_value)| {
                                            let param = descriptor.params.get(param_idx)?;
                                            let value = param.clamp(param.user_input_to_stored(user_value));
                                            Some(if name == "set-rack-slot-instrument-plock-batch" {
                                                app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                                    track,
                                                    slot_idx,
                                                    steps: steps.clone(),
                                                    param_idx,
                                                    value,
                                                }
                                            } else {
                                                app::AppCommand::SetRackSlotInstrumentParam {
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    value,
                                                }
                                            })
                                        }).collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default();
                                let gesture = map_string(map, "gesture")
                                    .unwrap_or_else(|| "rack-instrument".to_string());
                                let label = map_string(map, "label")
                                    .unwrap_or_else(|| "Set rack instrument parameters".to_string());
                                let result = if name == "set-rack-slot-instrument-plock-batch" {
                                    app::edit::apply_coalesced_device_plock_batch(
                                        &mut app,
                                        &commands,
                                        &gesture,
                                        &label,
                                    )
                                } else {
                                    app::edit::apply_coalesced_device_value_batch(
                                        &mut app,
                                        &commands,
                                        &gesture,
                                        &label,
                                    )
                                };
                                match result {
                                    Ok(_) => {
                                        rack_control_snapshot_dirty = true;
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Error(
                                        format!("rack instrument parameter batch failed: {error:?}"),
                                    )),
                                }
                            }
                        }
                    }
                    "set-rack-slot-instrument-param" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param_idx = map_usize(map, "param-idx");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(track), Some(slot_idx), Some(param_idx), Some(user_val)) =
                                (track, slot_idx, param_idx, value)
                            {
                                if let Some(slot) =
                                    rack_slot_snapshot_for_host(&state, track, slot_idx)
                                {
                                    if let Some(desc) = app
                                        .rack_slot_instrument_descriptor(&slot)
                                        .and_then(|desc| desc.params.get(param_idx).cloned())
                                    {
                                        let stored =
                                            desc.clamp(desc.user_input_to_stored(user_val));
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotInstrumentParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: stored,
                                            },
                                        );
                                        rack_control_snapshot_dirty = true;
                                        if param_change_needs_fx_rebuild(&desc) {
                                            refresh_instrument_panel_reactive(
                                                &mut editor,
                                                &app,
                                                track,
                                                &selected_steps,
                                                &ui_epoch,
                                            );
                                        } else {
                                            refresh_rack_direct_param_reactive(
                                                &mut editor,
                                                &app,
                                                &state,
                                                track,
                                                RackDirectDisplayTarget::InstrumentParam {
                                                    slot_idx,
                                                    param_idx,
                                                },
                                                &selected_steps,
                                                false,
                                                &ui_epoch,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-rack-slot-instrument-plock" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param_idx = map_usize(map, "param-idx");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(track), Some(slot_idx), Some(param_idx), Some(user_val)) =
                                (track, slot_idx, param_idx, value)
                            {
                                if let Some(slot) =
                                    rack_slot_snapshot_for_host(&state, track, slot_idx)
                                {
                                    if let Some(desc) = app
                                        .rack_slot_instrument_descriptor(&slot)
                                        .and_then(|desc| desc.params.get(param_idx).cloned())
                                    {
                                        let stored =
                                            desc.clamp(desc.user_input_to_stored(user_val));
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                                track,
                                                slot_idx,
                                                steps,
                                                param_idx,
                                                value: stored,
                                            },
                                        );
                                        rack_control_snapshot_dirty = true;
                                        if param_change_needs_fx_rebuild(&desc) {
                                            sync_rack_slot_instrument_authoring_display(
                                                &mut editor,
                                                &app,
                                                &state,
                                                track,
                                                &selected_steps,
                                            );
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        } else {
                                            refresh_rack_direct_param_reactive(
                                                &mut editor,
                                                &app,
                                                &state,
                                                track,
                                                RackDirectDisplayTarget::InstrumentParam {
                                                    slot_idx,
                                                    param_idx,
                                                },
                                                &selected_steps,
                                                true,
                                                &ui_epoch,
                                            );
                                        }
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "toggle-rack-slot-instrument-param" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param_idx = map_usize(map, "param-idx");
                            if let (Some(track), Some(slot_idx), Some(param_idx)) =
                                (track, slot_idx, param_idx)
                            {
                                if let Some(slot) =
                                    rack_slot_snapshot_for_host(&state, track, slot_idx)
                                {
                                    if let Some(desc) = app
                                        .rack_slot_instrument_descriptor(&slot)
                                        .and_then(|desc| desc.params.get(param_idx).cloned())
                                    {
                                        let current = slot
                                            .instrument_slot
                                            .defaults
                                            .get(param_idx)
                                            .copied()
                                            .unwrap_or(desc.default);
                                        let next =
                                            desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotInstrumentParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: next,
                                            },
                                        );
                                        refresh_instrument_panel_reactive(
                                            &mut editor,
                                            &app,
                                            track,
                                            &selected_steps,
                                            &ui_epoch,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    "toggle-rack-slot-instrument-plock" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param_idx = map_usize(map, "param-idx");
                            if let (Some(track), Some(slot_idx), Some(param_idx)) =
                                (track, slot_idx, param_idx)
                            {
                                if let Some(slot) =
                                    rack_slot_snapshot_for_host(&state, track, slot_idx)
                                {
                                    if let Some(desc) = app
                                        .rack_slot_instrument_descriptor(&slot)
                                        .and_then(|desc| desc.params.get(param_idx).cloned())
                                    {
                                        let selected: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        let default = slot
                                            .instrument_slot
                                            .defaults
                                            .get(param_idx)
                                            .copied()
                                            .unwrap_or(desc.default);
                                        let current = selected
                                            .iter()
                                            .copied()
                                            .min()
                                            .and_then(|step| {
                                                slot.instrument_slot
                                                    .plocks
                                                    .get(step)
                                                    .and_then(|step_plocks| {
                                                        step_plocks.get(param_idx)
                                                    })
                                                    .copied()
                                                    .flatten()
                                            })
                                            .unwrap_or(default);
                                        let next =
                                            desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                                track,
                                                slot_idx,
                                                steps: selected,
                                                param_idx,
                                                value: next,
                                            },
                                        );
                                        sync_rack_slot_instrument_authoring_display(
                                            &mut editor,
                                            &app,
                                            &state,
                                            track,
                                            &selected_steps,
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-rack-slot-instrument-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param_idx = map_usize(map, "param-idx");
                            let label = map_string(map, "label");
                            if let (Some(track), Some(slot_idx), Some(param_idx), Some(label)) =
                                (track, slot_idx, param_idx, label)
                            {
                                if let Some(slot) =
                                    rack_slot_snapshot_for_host(&state, track, slot_idx)
                                {
                                    if let Some(sequencer::effects::ParamKind::Enum { labels }) =
                                        app.rack_slot_instrument_descriptor(&slot).and_then(
                                            |desc| {
                                                desc.params
                                                    .get(param_idx)
                                                    .map(|param| param.kind.clone())
                                            },
                                        )
                                    {
                                        if let Some(selected_idx) =
                                            labels.iter().position(|item| item == &label)
                                        {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetRackSlotInstrumentParam {
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    value: selected_idx as f32,
                                                },
                                            );
                                            refresh_instrument_panel_reactive(
                                                &mut editor,
                                                &app,
                                                track,
                                                &selected_steps,
                                                &ui_epoch,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-rack-slot-instrument-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let track = map_usize(map, "track");
                            let slot_idx = map_usize(map, "slot");
                            let param_idx = map_usize(map, "param-idx");
                            let label = map_string(map, "label");
                            if let (Some(track), Some(slot_idx), Some(param_idx), Some(label)) =
                                (track, slot_idx, param_idx, label)
                            {
                                if let Some(slot) =
                                    rack_slot_snapshot_for_host(&state, track, slot_idx)
                                {
                                    if let Some(sequencer::effects::ParamKind::Enum { labels }) =
                                        app.rack_slot_instrument_descriptor(&slot).and_then(
                                            |desc| {
                                                desc.params
                                                    .get(param_idx)
                                                    .map(|param| param.kind.clone())
                                            },
                                        )
                                    {
                                        if let Some(selected_idx) =
                                            labels.iter().position(|item| item == &label)
                                        {
                                            let steps: Vec<usize> = selected_steps
                                                .lock()
                                                .unwrap()
                                                .iter()
                                                .copied()
                                                .collect();
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                                    track,
                                                    slot_idx,
                                                    steps,
                                                    param_idx,
                                                    value: selected_idx as f32,
                                                },
                                            );
                                            sync_rack_slot_instrument_authoring_display(
                                                &mut editor,
                                                &app,
                                                &state,
                                                track,
                                                &selected_steps,
                                            );
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-param-batch"
                    | "set-instrument-plock-batch"
                    | "set-instrument-key-lock-batch" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(updates) = map_param_updates(map) {
                                let track = map_usize(map, "track")
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                                let mut commands = Vec::with_capacity(updates.len());
                                let steps = map_usize_list(map, "steps").unwrap_or_else(|| {
                                    selected_steps
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .copied()
                                        .collect::<Vec<_>>()
                                });
                                let notes = map_u8_list(map, "notes").unwrap_or_default();
                                for (param_idx, user_value) in updates {
                                    if let Some(desc) = app
                                        .graph
                                        .instrument_descriptors
                                        .get(track)
                                        .and_then(|descriptor| descriptor.params.get(param_idx))
                                    {
                                        let value = desc.clamp(desc.user_input_to_stored(user_value));
                                        commands.push(if name == "set-instrument-plock-batch" {
                                            app::AppCommand::SetInstrumentPlockMulti {
                                                track,
                                                steps: steps.clone(),
                                                param_idx,
                                                value,
                                            }
                                        } else if name == "set-instrument-key-lock-batch" {
                                            app::AppCommand::SetInstrumentKeyLockMulti {
                                                track,
                                                notes: notes.clone(),
                                                param_idx,
                                                value,
                                            }
                                        } else {
                                            app::AppCommand::SetInstrumentParam {
                                                track,
                                                param_idx,
                                                value,
                                            }
                                        });
                                    }
                                }
                                let result = if name == "set-instrument-plock-batch" {
                                    let gesture = map_string(map, "gesture")
                                        .unwrap_or_else(|| "instrument-envelope".to_string());
                                    let label = map_string(map, "label")
                                        .unwrap_or_else(|| "Set instrument envelope".to_string());
                                    app::edit::apply_coalesced_device_plock_batch(
                                        &mut app,
                                        &commands,
                                        &gesture,
                                        &label,
                                    )
                                } else {
                                    let gesture = map_string(map, "gesture")
                                        .unwrap_or_else(|| "instrument-envelope".to_string());
                                    let label = map_string(map, "label")
                                        .unwrap_or_else(|| "Set instrument envelope".to_string());
                                    app::edit::apply_coalesced_device_value_batch(
                                        &mut app,
                                        &commands,
                                        &gesture,
                                        &label,
                                    )
                                };
                                if result.is_ok() {
                                    let plocks_changed = name == "set-instrument-plock-batch";
                                    let display_step = if plocks_changed {
                                        displayed_plock_step(
                                            &state,
                                            track,
                                            selected_plock_step(&selected_steps),
                                        )
                                    } else {
                                        None
                                    };
                                    let param_indices = commands
                                        .iter()
                                        .filter_map(|command| match command {
                                            app::AppCommand::SetInstrumentParam { param_idx, .. }
                                            | app::AppCommand::SetInstrumentPlockMulti {
                                                param_idx, ..
                                            }
                                            | app::AppCommand::SetInstrumentKeyLockMulti {
                                                param_idx, ..
                                            } => Some(*param_idx),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>();
                                    let neural_selection =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    sync_instrument_param_batch_display(
                                        &mut editor,
                                        &app,
                                        &state,
                                        &selected_steps,
                                        &neural_selection,
                                        track,
                                        &param_indices,
                                        display_step,
                                        plocks_changed,
                                    );
                                }
                                match result {
                                    Ok(_) if map_bool(map, "commit") => {
                                        app::edit::finish_active_gesture(&mut app);
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Ok(_) => {}
                                    Err(error) => editor.handle_host_event(HostEvent::Error(
                                        format!("instrument parameter batch failed: {error:?}"),
                                    )),
                                }
                            }
                        }
                    }
                    "set-instrument-param" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(user_val)) = (param_idx, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                    let (neural_selection, wrote_neural_plock, neural_history_before) =
                                        record_selected_neural_instrument_plock(
                                            &mut editor,
                                            &state,
                                            &selected_neural_neurons,
                                            track,
                                            param_idx,
                                            stored,
                                        );
                                    if let Some(before) = neural_history_before {
                                        app.commit_applied_scene_structure_mutation(
                                            before,
                                            "Edit neural override",
                                        );
                                    }
                                    if !wrote_neural_plock {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentParam {
                                                track,
                                                param_idx,
                                                value: stored,
                                            },
                                        );
                                    }
                                    sync_instrument_param_authoring_display(
                                        &mut editor,
                                        InstrumentParamDisplaySync {
                                            app: &app,
                                            state: &state,
                                            selected_steps: &selected_steps,
                                            selection: &neural_selection,
                                            track,
                                            param_idx,
                                            display_step: None,
                                            sync_plock_list: wrote_neural_plock,
                                            sync_plock_presence: false,
                                            sync_sampler_times: true,
                                        },
                                    );
                                    if param_change_needs_fx_rebuild(&desc) {
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "audition-instrument-key" => {
                        if let Value::Map(ref map) = payload {
                            let note =
                                map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                            if let Some(note) = note {
                                let track = current_track.load(Ordering::Relaxed);
                                if track < app.tracks.len() {
                                    let base_note_offset = f32::from_bits(
                                        app.state.pattern.instrument_base_note_offsets[track]
                                            .load(Ordering::Relaxed),
                                    );
                                    let transpose = note as f32 - 60.0 - base_note_offset;
                                    release_matching_key_lock_auditions(
                                        &mut pending_key_lock_auditions,
                                        &keyboard_tx,
                                        track,
                                        transpose,
                                    );
                                    if keyboard_tx
                                        .send(KeyboardTrigger {
                                            track,
                                            transpose,
                                            velocity: 1.0,
                                            note_off: false,
                                        })
                                        .is_ok()
                                    {
                                        pending_key_lock_auditions.push(PendingKeyLockAudition {
                                            track,
                                            transpose,
                                            release_at: Instant::now() + KEY_LOCK_AUDITION_DURATION,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-key-lock" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx = map_usize(map, "param-idx");
                            let note =
                                map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(param_idx), Some(note), Some(user_val)) =
                                (param_idx, note, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetInstrumentKeyLock {
                                            track,
                                            note,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-key-lock-multi" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx = map_usize(map, "param-idx");
                            let notes = map_u8_list(map, "notes").filter(|notes| !notes.is_empty());
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(param_idx), Some(notes), Some(user_val)) =
                                (param_idx, notes, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetInstrumentKeyLockMulti {
                                            track,
                                            notes,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-key-lock-option" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx = map_usize(map, "param-idx");
                            let note =
                                map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                            let label = map_string(map, "label");
                            if let (Some(param_idx), Some(note), Some(label)) =
                                (param_idx, note, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .map(|d| &d.kind)
                                {
                                    if let Some(selected_idx) =
                                        labels.iter().position(|item| item == &label)
                                    {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentKeyLock {
                                                track,
                                                note,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-key-lock-option-multi" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx = map_usize(map, "param-idx");
                            let notes = map_u8_list(map, "notes").filter(|notes| !notes.is_empty());
                            let label = map_string(map, "label");
                            if let (Some(param_idx), Some(notes), Some(label)) =
                                (param_idx, notes, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .map(|d| &d.kind)
                                {
                                    if let Some(selected_idx) =
                                        labels.iter().position(|item| item == &label)
                                    {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentKeyLockMulti {
                                                track,
                                                notes,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "clear-instrument-key-lock" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx = map_usize(map, "param-idx");
                            let note =
                                map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                            if let (Some(param_idx), Some(note)) = (param_idx, note) {
                                let track = current_track.load(Ordering::Relaxed);
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::ClearInstrumentKeyLock {
                                        track,
                                        note,
                                        param_idx,
                                    },
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "clear-instrument-key-locks-for-note" => {
                        if let Value::Map(ref map) = payload {
                            let note =
                                map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                            if let Some(note) = note {
                                let track = current_track.load(Ordering::Relaxed);
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::ClearInstrumentKeyLocksForNote { track, note },
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "stamp-key-lock-variant" => {
                        if let Value::Map(ref map) = payload {
                            let notes = map_u8_list(map, "notes").filter(|notes| !notes.is_empty());
                            let label = map_string(map, "label");
                            if let (Some(notes), Some(label)) = (notes, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                let applied = if label == "def" {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::ClearInstrumentKeyLockVariantsForNotes {
                                            track,
                                            notes,
                                        },
                                    );
                                    true
                                } else {
                                    state
                                        .key_lock_variant_registry_snapshot(track)
                                        .assignment_for_label(&label)
                                        .is_some_and(|assignment| {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::StampInstrumentKeyLockVariant {
                                                    track,
                                                    notes,
                                                    key: assignment.key,
                                                },
                                            );
                                            true
                                        })
                                };
                                if applied {
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "toggle-instrument-param" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let Some(param_idx) = param_idx {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let slot = &app.state.pattern.instrument_slots[track];
                                    let selected: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    let neural_selection =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    let default = if param_idx
                                        < slot.num_params.load(Ordering::Relaxed) as usize
                                    {
                                        slot.defaults.get(param_idx)
                                    } else {
                                        desc.default
                                    };
                                    let current = sequencer::lisp_host::selected_neural_instrument_plock_value(
                                        &state,
                                        &neural_selection,
                                        track,
                                        param_idx,
                                    )
                                    .or_else(|| {
                                        selected
                                            .iter()
                                            .copied()
                                            .min()
                                            .and_then(|step| slot.plocks.get(step, param_idx))
                                    })
                                    .unwrap_or(default);
                                    let next = desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                    let neural_history_before = (!neural_selection.is_empty())
                                        .then(|| state.capture_project_scenes());
                                    let wrote_neural_plock = write_selected_neural_instrument_plock(
                                        &mut editor,
                                        &state,
                                        &neural_selection,
                                        track,
                                        param_idx,
                                        next,
                                    );
                                    if let Some(before) =
                                        neural_history_before.filter(|_| wrote_neural_plock)
                                    {
                                        app.commit_applied_scene_structure_mutation(
                                            before,
                                            "Edit neural override",
                                        );
                                    }
                                    if wrote_neural_plock {
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: true,
                                                sync_plock_presence: false,
                                                sync_sampler_times: false,
                                            },
                                        );
                                    } else if selected.is_empty() {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentParam {
                                                track,
                                                param_idx,
                                                value: next,
                                            },
                                        );
                                        if sync_instrument_param_value_field(
                                            editor.runtime_mut(),
                                            &app,
                                            track,
                                            param_idx,
                                            None,
                                        ) {
                                            editor.mark_needs_redraw();
                                        }
                                    } else {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentPlockMulti {
                                                track,
                                                steps: selected,
                                                param_idx,
                                                value: next,
                                            },
                                        );
                                        let display_step = displayed_plock_step(
                                            &state,
                                            track,
                                            selected_plock_step(&selected_steps),
                                        );
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step,
                                                sync_plock_list: false,
                                                sync_plock_presence: true,
                                                sync_sampler_times: false,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-effect-param-batch" | "set-effect-plock-batch" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx = map_usize(map, "slot-idx");
                            if let (Some(slot_idx), Some(updates)) =
                                (slot_idx, map_param_updates(map))
                            {
                                let track = map_usize(map, "track")
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                                let steps = map_usize_list(map, "steps").unwrap_or_else(|| {
                                    selected_steps
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .copied()
                                        .collect::<Vec<_>>()
                                });
                                let commands = updates
                                    .into_iter()
                                    .filter_map(|(param_idx, value)| {
                                        let desc = app
                                            .graph
                                            .effect_descriptors
                                            .get(track)?
                                            .get(slot_idx)?
                                            .params
                                            .get(param_idx)?;
                                        let value = value.clamp(desc.min, desc.max);
                                        Some(if name == "set-effect-plock-batch" {
                                            app::AppCommand::SetEffectPlockMulti {
                                                track,
                                                steps: steps.clone(),
                                                slot_idx,
                                                param_idx,
                                                value,
                                            }
                                        } else {
                                            app::AppCommand::SetEffectParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value,
                                            }
                                        })
                                    })
                                    .collect::<Vec<_>>();
                                let result = if name == "set-effect-plock-batch" {
                                    let gesture = map_string(map, "gesture")
                                        .unwrap_or_else(|| "effect-curve".to_string());
                                    let label = map_string(map, "label")
                                        .unwrap_or_else(|| "Set effect curve".to_string());
                                    app::edit::apply_coalesced_device_plock_batch(
                                        &mut app,
                                        &commands,
                                        &gesture,
                                        &label,
                                    )
                                } else {
                                    app::edit::apply_coalesced_device_value_batch(
                                        &mut app,
                                        &commands,
                                        "effect-curve",
                                        "Set effect curve",
                                    )
                                };
                                if result.is_ok() {
                                    let plocks_changed = name == "set-effect-plock-batch";
                                    let display_step = if plocks_changed {
                                        displayed_plock_step(
                                            &state,
                                            track,
                                            selected_plock_step(&selected_steps),
                                        )
                                    } else {
                                        None
                                    };
                                    let param_indices = commands
                                        .iter()
                                        .filter_map(|command| match command {
                                            app::AppCommand::SetEffectParam { param_idx, .. }
                                            | app::AppCommand::SetEffectPlockMulti {
                                                param_idx, ..
                                            } => Some(*param_idx),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>();
                                    let neural_selection =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    sync_effect_param_batch_display(
                                        &mut editor,
                                        &app,
                                        &neural_selection,
                                        track,
                                        slot_idx,
                                        &param_indices,
                                        display_step,
                                    );
                                }
                                match result {
                                    Ok(_) if map_bool(map, "commit") => {
                                        app::edit::finish_active_gesture(&mut app);
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Ok(_) => {}
                                    Err(error) => editor.handle_host_event(HostEvent::Error(
                                        format!("effect parameter batch failed: {error:?}"),
                                    )),
                                }
                            }
                        }
                    }
                    "set-effect-param" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(value)) =
                                (slot_idx, param_idx, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let desc = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                    .and_then(|desc| desc.params.get(param_idx))
                                    .cloned();
                                let clamped = desc
                                    .as_ref()
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                let (neural_selection, wrote_neural_plock, neural_history_before) =
                                    record_selected_neural_effect_plock(
                                        &mut editor,
                                        &state,
                                        &selected_neural_neurons,
                                        track,
                                        slot_idx,
                                        param_idx,
                                    clamped,
                                );
                                if let Some(before) = neural_history_before {
                                    app.commit_applied_scene_structure_mutation(
                                        before,
                                        "Edit neural override",
                                    );
                                }
                                if !wrote_neural_plock {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetEffectParam {
                                            track,
                                            slot_idx,
                                            param_idx,
                                            value: clamped,
                                        },
                                    );
                                }
                                sync_effect_param_authoring_display(
                                    &mut editor,
                                    EffectParamDisplaySync {
                                        state: &state,
                                        effect_descriptors: &app.graph.effect_descriptors,
                                        app: &app,
                                        selected_steps: &selected_steps,
                                        selection: &neural_selection,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        display_step: None,
                                        sync_plock_list: wrote_neural_plock,
                                    },
                                );
                                if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "toggle-effect-param" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let (Some(slot_idx), Some(param_idx)) = (slot_idx, param_idx) {
                                let selected: Vec<usize> =
                                    selected_steps.lock().unwrap().iter().copied().collect();
                                if map_bool(map, "bus-fx") {
                                    let bus_idx =
                                        map.get("bus").and_then(|cell| match &*cell.borrow() {
                                            Value::Number(n) => Some(*n as usize),
                                            _ => None,
                                        });
                                    if let Some(bus_idx) = bus_idx {
                                        let desc = app
                                            .buses
                                            .get(bus_idx)
                                            .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                            .and_then(|desc| desc.params.get(param_idx))
                                            .cloned();
                                        if let Some(desc) = desc {
                                            let current = app
                                                .buses
                                                .get(bus_idx)
                                                .and_then(|bus| bus.effect_slots.get(slot_idx))
                                                .map(|slot| {
                                                    let default = slot
                                                        .defaults
                                                        .get(param_idx)
                                                        .copied()
                                                        .unwrap_or(desc.default);
                                                    selected
                                                        .iter()
                                                        .copied()
                                                        .min()
                                                        .and_then(|step| {
                                                            slot.plocks
                                                                .get(step)
                                                                .and_then(|step_plocks| {
                                                                    step_plocks.get(param_idx)
                                                                })
                                                                .copied()
                                                                .flatten()
                                                        })
                                                        .unwrap_or(default)
                                                })
                                                .unwrap_or(desc.default);
                                            let next =
                                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                            if selected.is_empty() {
                                                match app.apply_recorded_bus_effect_value_mutation(
                                                    bus_idx,
                                                    slot_idx,
                                                    "Set bus effect parameter",
                                                    format!("param:{param_idx}"),
                                                    |app| app.set_bus_effect_param(
                                                        bus_idx, slot_idx, param_idx, next,
                                                    ),
                                                ) {
                                                    Ok(()) => {
                                                        app.publish_bus_gate_runtime();
                                                        *bus_state.lock().unwrap() =
                                                            app.buses.clone();
                                                        if sync_bus_effect_param_value_field(
                                                            editor.runtime_mut(),
                                                            &app,
                                                            bus_idx,
                                                            slot_idx,
                                                            param_idx,
                                                        ) {
                                                            editor.mark_needs_redraw();
                                                        }
                                                    }
                                                    Err(error) => {
                                                        editor.handle_host_event(
                                                            HostEvent::Status(format!(
                                                                "Error toggling bus effect param: {error}"
                                                            )),
                                                        );
                                                        continue;
                                                    }
                                                }
                                            } else {
                                                let result = app.apply_recorded_bus_effect_value_mutation(
                                                    bus_idx,
                                                    slot_idx,
                                                    "Set bus effect p-lock",
                                                    format!("plock:param:{param_idx}"),
                                                    |app| {
                                                        for step in selected {
                                                            app.set_bus_effect_plock(
                                                                bus_idx, slot_idx, step, param_idx, next,
                                                            )?;
                                                        }
                                                        Ok(())
                                                    },
                                                );
                                                if result.is_ok() {
                                                    app.publish_bus_gate_runtime();
                                                    *bus_state.lock().unwrap() = app.buses.clone();
                                                } else if let Err(error) = result {
                                                    editor.handle_host_event(HostEvent::Status(format!(
                                                        "Error toggling bus effect p-lock: {error}"
                                                    )));
                                                }
                                            }
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                } else if map_bool(map, "midi-fx") {
                                    let track = current_track.load(Ordering::Relaxed);
                                    let chain = state.pattern.track_params[track].midi_fx_chain();
                                    let desc = chain
                                        .get(slot_idx)
                                        .and_then(|name| {
                                            sequencer::lisp_host::load_midi_fx_descriptor(name)
                                        })
                                        .and_then(|desc| desc.params.get(param_idx).cloned());
                                    if let Some(desc) = desc {
                                        if let Some(slot) = state
                                            .pattern
                                            .midi_fx_slots
                                            .get(track)
                                            .and_then(|slots| slots.get(slot_idx))
                                        {
                                            let default = slot.defaults.get(param_idx);
                                            let current = selected
                                                .iter()
                                                .copied()
                                                .min()
                                                .and_then(|step| slot.plocks.get(step, param_idx))
                                                .unwrap_or(default);
                                            let next =
                                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                            if selected.is_empty() {
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetMidiFxParam {
                                                        track,
                                                        slot_idx,
                                                        param_idx,
                                                        value: next,
                                                    },
                                                );
                                                if sync_midi_fx_param_value_field(
                                                    editor.runtime_mut(),
                                                    &state,
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    None,
                                                ) {
                                                    editor.mark_needs_redraw();
                                                }
                                            } else {
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetMidiFxPlockMulti {
                                                        track,
                                                        steps: selected,
                                                        slot_idx,
                                                        param_idx,
                                                        value: next,
                                                    },
                                                );
                                            }
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                } else {
                                    let track = current_track.load(Ordering::Relaxed);
                                    let desc = app
                                        .graph
                                        .effect_descriptors
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                        .and_then(|desc| desc.params.get(param_idx))
                                        .cloned();
                                    if let Some(desc) = desc {
                                        let chain = &state.pattern.effect_chains[track];
                                        let neural_selection =
                                            selected_neural_neurons.lock().unwrap().clone();
                                        let current = chain
                                            .get(slot_idx)
                                            .map(|slot| {
                                                let default = slot.defaults.get(param_idx);
                                                sequencer::lisp_host::selected_neural_effect_plock_value(
                                                    &state,
                                                    &neural_selection,
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                )
                                                .or_else(|| {
                                                    selected
                                                        .iter()
                                                        .copied()
                                                        .min()
                                                        .and_then(|step| {
                                                            slot.plocks.get(step, param_idx)
                                                        })
                                                })
                                                .unwrap_or(default)
                                            })
                                            .unwrap_or(desc.default);
                                        let next =
                                            desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                        let neural_history_before = (!neural_selection.is_empty())
                                            .then(|| state.capture_project_scenes());
                                        let wrote_neural_plock = write_selected_neural_effect_plock(
                                            &mut editor,
                                            &state,
                                            &neural_selection,
                                            track,
                                            slot_idx,
                                            param_idx,
                                            next,
                                        );
                                        if let Some(before) =
                                            neural_history_before.filter(|_| wrote_neural_plock)
                                        {
                                            app.commit_applied_scene_structure_mutation(
                                                before,
                                                "Edit neural override",
                                            );
                                        }
                                        if wrote_neural_plock {
                                            sync_effect_param_authoring_display(
                                                &mut editor,
                                                EffectParamDisplaySync {
                                                    state: &state,
                                                    effect_descriptors: &app
                                                        .graph
                                                        .effect_descriptors,
                                                    app: &app,
                                                    selected_steps: &selected_steps,
                                                    selection: &neural_selection,
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    display_step: None,
                                                    sync_plock_list: true,
                                                },
                                            );
                                        } else if selected.is_empty() {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetEffectParam {
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    value: next,
                                                },
                                            );
                                            if sync_track_effect_param_value_field(
                                                editor.runtime_mut(),
                                                &app,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                None,
                                            ) {
                                                editor.mark_needs_redraw();
                                            }
                                        } else {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetEffectPlockMulti {
                                                    track,
                                                    slot_idx,
                                                    steps: selected,
                                                    param_idx,
                                                    value: next,
                                                },
                                            );
                                        }
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(label)) = (param_idx, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .map(|d| &d.kind)
                                {
                                    if let Some(selected_idx) =
                                        labels.iter().position(|item| item == &label)
                                    {
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock, neural_history_before) =
                                            record_selected_neural_instrument_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                param_idx,
                                            value,
                                        );
                                        if let Some(before) = neural_history_before {
                                            app.commit_applied_scene_structure_mutation(
                                                before,
                                                "Edit neural override",
                                            );
                                        }
                                        if !wrote_neural_plock {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetInstrumentParam {
                                                    track,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                                sync_plock_presence: false,
                                                sync_sampler_times: false,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-plock" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(user_val)) = (param_idx, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                    let (neural_selection, wrote_neural_plock, neural_history_before) =
                                        record_selected_neural_instrument_plock(
                                            &mut editor,
                                            &state,
                                            &selected_neural_neurons,
                                            track,
                                            param_idx,
                                        stored,
                                    );
                                    if let Some(before) = neural_history_before {
                                        app.commit_applied_scene_structure_mutation(
                                            before,
                                            "Edit neural override",
                                        );
                                    }
                                    if !wrote_neural_plock {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentPlockMulti {
                                                track,
                                                steps,
                                                param_idx,
                                                value: stored,
                                            },
                                        );
                                    }
                                    let display_step = displayed_plock_step(
                                        &state,
                                        track,
                                        selected_plock_step(&selected_steps),
                                    );
                                    sync_instrument_param_authoring_display(
                                        &mut editor,
                                        InstrumentParamDisplaySync {
                                            app: &app,
                                            state: &state,
                                            selected_steps: &selected_steps,
                                            selection: &neural_selection,
                                            track,
                                            param_idx,
                                            display_step,
                                            sync_plock_list: wrote_neural_plock,
                                            sync_plock_presence: !wrote_neural_plock,
                                            sync_sampler_times: true,
                                        },
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-tensor-cell" => {
                        if let Value::Map(ref map) = payload {
                            let tensor_idx = map_usize(map, "tensor-idx");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(tensor_idx), Some(user_val)) = (tensor_idx, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(tensor_desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|desc| desc.tensor_params.get(tensor_idx))
                                    .cloned()
                                {
                                    let cell_idx = map_usize(map, "cell-idx").or_else(|| {
                                        let row = map_usize(map, "row")?;
                                        let col = map_usize(map, "col")?;
                                        (col < tensor_desc.cols())
                                            .then_some(row * tensor_desc.cols() + col)
                                    });
                                    let Some(cell_idx) = cell_idx else {
                                        continue;
                                    };
                                    if cell_idx >= tensor_desc.default.len() {
                                        continue;
                                    }
                                    let value = user_val.clamp(tensor_desc.min, tensor_desc.max);
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    if steps.is_empty() {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentTensorCell {
                                                track,
                                                tensor_idx,
                                                cell_idx,
                                                value,
                                            },
                                        );
                                    } else {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetInstrumentTensorPlockCellMulti {
                                                track,
                                                steps,
                                                tensor_idx,
                                                cell_idx,
                                                value,
                                            },
                                        );
                                    }
                                    let display_step = displayed_plock_step(
                                        &state,
                                        track,
                                        selected_plock_step(&selected_steps),
                                    );
                                    if sync_instrument_tensor_value_field(
                                        editor.runtime_mut(),
                                        &app,
                                        track,
                                        tensor_idx,
                                        display_step,
                                    ) {
                                        editor.refresh_runtime_side_effects();
                                        editor.mark_needs_redraw();
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(label)) = (param_idx, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .map(|d| &d.kind)
                                {
                                    if let Some(selected_idx) =
                                        labels.iter().position(|item| item == &label)
                                    {
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock, neural_history_before) =
                                            record_selected_neural_instrument_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                param_idx,
                                            value,
                                        );
                                        if let Some(before) = neural_history_before {
                                            app.commit_applied_scene_structure_mutation(
                                                before,
                                                "Edit neural override",
                                            );
                                        }
                                        if !wrote_neural_plock {
                                            let steps: Vec<usize> = selected_steps
                                                .lock()
                                                .unwrap()
                                                .iter()
                                                .copied()
                                                .collect();
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetInstrumentPlockMulti {
                                                    track,
                                                    steps,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                        let display_step = displayed_plock_step(
                                            &state,
                                            track,
                                            selected_plock_step(&selected_steps),
                                        );
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step,
                                                sync_plock_list: wrote_neural_plock,
                                                sync_plock_presence: !wrote_neural_plock,
                                                sync_sampler_times: false,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-effect-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(label)) =
                                (slot_idx, param_idx, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let selected_idx = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|d| d.get(slot_idx))
                                    .and_then(|d| d.params.get(param_idx))
                                    .and_then(|p| match &p.kind {
                                        sequencer::effects::ParamKind::Enum { labels } => {
                                            labels.iter().position(|item| item == &label)
                                        }
                                        _ => None,
                                    })
                                    .or_else(|| {
                                        let is_delay_time = app
                                            .graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .map(|d| d.name == "Delay")
                                            .unwrap_or(false)
                                            && param_idx == 2;
                                        is_delay_time.then(|| {
                                            sequencer::effects::SyncDivision::ALL
                                                .iter()
                                                .position(|div| div.label() == label)
                                        })?
                                    });
                                if let Some(selected_idx) = selected_idx {
                                    let is_host_sidechain = matches!(
                                        app.graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .and_then(|d| d.params.get(param_idx))
                                            .and_then(|p| p.host_control.as_ref()),
                                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                                    );
                                    if is_host_sidechain {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetEffectParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    } else {
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock, neural_history_before) =
                                            record_selected_neural_effect_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                slot_idx,
                                                param_idx,
                                            value,
                                        );
                                        if let Some(before) = neural_history_before {
                                            app.commit_applied_scene_structure_mutation(
                                                before,
                                                "Edit neural override",
                                            );
                                        }
                                        if !wrote_neural_plock {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetEffectParam {
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                        sync_effect_param_authoring_display(
                                            &mut editor,
                                            EffectParamDisplaySync {
                                                state: &state,
                                                effect_descriptors: &app.graph.effect_descriptors,
                                                app: &app,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "group-selected-tracks" => {
                        // Fold the multi-selected tracks into a new group backed by
                        // an auto-created bus. Reject if <2 tracks or any member is
                        // already grouped (one group per track).
                        let members: Vec<usize> = {
                            let set = selected_tracks.lock().unwrap();
                            let mut v: Vec<usize> = set
                                .iter()
                                .copied()
                                .filter(|&t| t < app.tracks.len())
                                .collect();
                            v.sort_unstable();
                            v
                        };
                        let already_grouped = members
                            .iter()
                            .any(|m| app.groups.iter().any(|g| g.members.contains(m)));
                        if members.len() >= 2 && !already_grouped {
                            let Ok(bus) = app.group_tracks_recorded(members.clone()) else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Could not group the selected tracks".to_string(),
                                ));
                                continue;
                            };
                            let selected_bus_index = app
                                .buses
                                .iter()
                                .position(|candidate| candidate.id == bus)
                                .expect("new group backing bus must be present in app buses");
                            selected_tracks.lock().unwrap().clear();
                            *bus_state.lock().unwrap() = app.buses.clone();
                            *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                            *track_groups.lock().unwrap() = app.groups.clone();
                            let ct = current_track.load(Ordering::Relaxed);
                            let rt = editor.runtime_mut();
                            sync_track_mixer_state(rt, &app, &state);
                            sync_bus_mixer_state(rt, &app);
                            sync_groups_bindings(rt, &app.groups);
                            sync_selected_tracks_bindings(
                                rt,
                                app.tracks.len(),
                                ct,
                                &HashSet::new(),
                            );
                            let _ =
                                rt.eval_str(&format!("(set! selected-bus {selected_bus_index})"));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    "move-track-to-group" => {
                        // Drag-drop: add `track` to the group at `gidx` (moving it
                        // out of any other group first). Dissolve a source group
                        // that would drop below 2 members.
                        let track = extract_usize_from_payload(&payload, "track");
                        let gidx = extract_usize_from_payload(&payload, "gidx");
                        if let (Some(track), Some(gidx)) = (track, gidx) {
                            if app.move_track_to_group_recorded(track, gidx).is_ok() {
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                                    *track_groups.lock().unwrap() = app.groups.clone();
                                    let rt = editor.runtime_mut();
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_bus_mixer_state(rt, &app);
                                    sync_groups_bindings(rt, &app.groups);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "remove-track-from-group" => {
                        // Drag-drop onto the sample zone: pull `track` out of its
                        // group, routing it back to the master mix. Dissolve the
                        // group if it would fall below 2 members.
                        if let Some(track) = extract_usize_from_payload(&payload, "track") {
                            if app.remove_track_from_group_recorded(track).is_ok() {
                                *bus_state.lock().unwrap() = app.buses.clone();
                                *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                                *track_groups.lock().unwrap() = app.groups.clone();
                                let rt = editor.runtime_mut();
                                sync_track_mixer_state(rt, &app, &state);
                                sync_bus_mixer_state(rt, &app);
                                sync_groups_bindings(rt, &app.groups);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "set-track-output" => {
                        if let Value::Map(ref map) = payload {
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let payload_track =
                                map.get("track").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let Some(label) = label {
                                let track = payload_track
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                                let output = if label == "main" {
                                    Some(TrackOutput::Mix)
                                } else if label == "sends only" {
                                    Some(TrackOutput::None)
                                } else {
                                    app.buses
                                        .iter()
                                        .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
                                        .find(|bus| bus.name == label)
                                        .map(|bus| TrackOutput::Bus(bus.id))
                                };
                                if let Some(output) = output {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetTrackOutput { track, output },
                                    );
                                    let rt = editor.runtime_mut();
                                    sync_track_mixer_state(rt, &app, &state);
                                    if track == current_track.load(Ordering::Relaxed) {
                                        let selected_neural_snapshot =
                                            selected_neural_neurons.lock().unwrap().clone();
                                        sync_track_params_with_neural_selection(
                                            rt,
                                            &app,
                                            &state,
                                            track,
                                            &selected_steps,
                                            Some(&selected_neural_snapshot),
                                        );
                                        sync_fx_param_binding_fields_with_neural_selection(
                                            rt,
                                            &app,
                                            &state,
                                            track,
                                            &selected_steps,
                                            Some(&selected_neural_snapshot),
                                        );
                                    }
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-mod-route" => {
                        if let Value::Map(ref map) = payload {
                            let source = map.get("source").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let dest = map.get("dest").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let destination =
                                match map.get("dest-kind").and_then(|cell| match &*cell.borrow() {
                                    Value::String(kind) => Some(kind.clone()),
                                    _ => None,
                                }) {
                                    Some(kind) if kind == "bus" => dest.map(|id| {
                                        sequencer::sequencer::ModDestination::Bus(
                                            sequencer::sequencer::BusId(id as u64),
                                        )
                                    }),
                                    _ => dest.map(sequencer::sequencer::ModDestination::Track),
                                };
                            let input = map
                                .get("input")
                                .and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            if let (Some(source), Some(destination)) = (source, destination) {
                                match app.apply_recorded_scene_structure_mutation(
                                    "Connect modulation route",
                                    |app| app.graph_controller().set_mod_route_to_destination(
                                        source,
                                        destination,
                                        input,
                                    ),
                                ) {
                                    Ok(()) => {
                                        let dest_label =
                                            mod_route_destination_status_label(&app, destination);
                                        let message = format!(
                                            "Connected mod route: track {} out -> {} Ext{}",
                                            source + 1,
                                            dest_label,
                                            input + 1
                                        );
                                        eprintln!("[mod-route] {message}");
                                        let rt = editor.runtime_mut();
                                        sync_track_mixer_state(rt, &app, &state);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(message));
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "[mod-route] rejected connect {} -> {:?}: {}",
                                            source + 1,
                                            destination,
                                            error
                                        );
                                        editor.handle_host_event(HostEvent::Status(error));
                                    }
                                }
                            }
                        }
                    }
                    "delete-mod-route" => {
                        if let Value::Map(ref map) = payload {
                            let source = map.get("source").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let dest = map.get("dest").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let destination =
                                match map.get("dest-kind").and_then(|cell| match &*cell.borrow() {
                                    Value::String(kind) => Some(kind.clone()),
                                    _ => None,
                                }) {
                                    Some(kind) if kind == "bus" => dest.map(|id| {
                                        sequencer::sequencer::ModDestination::Bus(
                                            sequencer::sequencer::BusId(id as u64),
                                        )
                                    }),
                                    _ => dest.map(sequencer::sequencer::ModDestination::Track),
                                };
                            let input = map
                                .get("input")
                                .and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            if let (Some(source), Some(destination)) = (source, destination) {
                                match app.apply_recorded_scene_structure_mutation(
                                    "Delete modulation route",
                                    |app| app.graph_controller().delete_mod_route_to_destination(
                                        source,
                                        destination,
                                        input,
                                    ),
                                ) {
                                    Ok(()) => {
                                        let dest_label =
                                            mod_route_destination_status_label(&app, destination);
                                        let message = format!(
                                            "Disconnected mod route: track {} out -> {} Ext{}",
                                            source + 1,
                                            dest_label,
                                            input + 1
                                        );
                                        eprintln!("[mod-route] {message}");
                                        let rt = editor.runtime_mut();
                                        sync_track_mixer_state(rt, &app, &state);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(message));
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "[mod-route] rejected disconnect {} -> {:?}: {}",
                                            source + 1,
                                            destination,
                                            error
                                        );
                                        editor.handle_host_event(HostEvent::Status(error));
                                    }
                                }
                            }
                        }
                    }
                    "refresh-mixer-ui" => {
                        let rt = editor.runtime_mut();
                        sync_track_mixer_state(rt, &app, &state);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    "set-track-bus-send" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let amount = map.get("amount").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            let payload_track =
                                map.get("track").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let (Some(bus_idx), Some(amount)) = (bus_idx, amount) {
                                let Some(bus) = app.buses.get(bus_idx) else {
                                    continue;
                                };
                                if bus.id == sequencer::sequencer::BusId::MIX {
                                    continue;
                                }
                                let track = payload_track
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                                if track >= state.active_track_count() {
                                    continue;
                                }
                                let mut sends = app.state.pattern.track_params[track].sends();
                                if let Some(send) =
                                    sends.iter_mut().find(|send| send.destination == bus.id)
                                {
                                    send.amount = amount;
                                } else {
                                    sends.push(TrackSendSnapshot {
                                        destination: bus.id,
                                        amount,
                                    });
                                }
                                sends.retain(|send| send.amount > 0.0);
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetTrackSends { track, sends },
                                );
                                let rt = editor.runtime_mut();
                                sync_track_bus_send_binding_field(rt, &app, &state, track, bus_idx);
                                let current = current_track.load(Ordering::Relaxed);
                                if track == current {
                                    sync_current_track_bus_send_binding_field(
                                        rt, &app, &state, track, bus_idx,
                                    );
                                }
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                            }
                        }
                    }
                    "toggle-bus-step" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    bus.gate_sequence.toggle_step(step);
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-bus-step-active" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            let active = map_bool(map, "active");
                            if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    if let Some(slot) = bus.gate_sequence.steps.get_mut(step) {
                                        if *slot != active {
                                            *slot = active;
                                            app.publish_bus_gate_runtime();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-step-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            let param = map_string(map, "param");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(bus_idx), Some(step), Some(param), Some(value)) =
                                (bus_idx, step, param, value)
                            {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    match param.as_str() {
                                        "duration" | "dur" => {
                                            bus.gate_sequence.set_step_duration(step, value);
                                        }
                                        "sync" | "syn" => {
                                            bus.gate_sequence.set_step_sync(step, value);
                                        }
                                        _ => {
                                            bus.gate_sequence.set_step_velocity(step, value);
                                        }
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-selected-bus-step-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let param = map_string(map, "param");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(bus_idx), Some(param), Some(value)) =
                                (bus_idx, param, value)
                            {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    for step in steps {
                                        if step >= bus.gate_sequence.num_steps {
                                            continue;
                                        }
                                        match param.as_str() {
                                            "duration" | "dur" => {
                                                bus.gate_sequence.set_step_duration(step, value);
                                            }
                                            "sync" | "syn" => {
                                                bus.gate_sequence.set_step_sync(step, value);
                                            }
                                            _ => {
                                                bus.gate_sequence.set_step_velocity(step, value);
                                            }
                                        }
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "select-bus-step-range" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let start = map_number(map, "start").map(|value| value as usize);
                            let end = map_number(map, "end").map(|value| value as usize);
                            if let (Some(bus_idx), Some(start), Some(end)) = (bus_idx, start, end) {
                                if let Some(bus) = app.buses.get(bus_idx) {
                                    let num_steps = bus.gate_sequence.num_steps.max(1);
                                    let a = start.min(num_steps - 1);
                                    let b = end.min(num_steps - 1);
                                    let lo = a.min(b);
                                    let hi = a.max(b);
                                    {
                                        let mut set = selected_steps.lock().unwrap();
                                        set.clear();
                                        set.extend(lo..=hi);
                                    }
                                    editor.runtime_mut().set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "select-bus-step" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                                if let Some(bus) = app.buses.get(bus_idx) {
                                    let num_steps = bus.gate_sequence.num_steps.max(1);
                                    let step = step.min(num_steps - 1);
                                    {
                                        let mut set = selected_steps.lock().unwrap();
                                        if !set.insert(step) {
                                            set.remove(&step);
                                        }
                                    }
                                    editor.runtime_mut().set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "select-all-bus-steps" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            if let Some(bus_idx) = bus_idx {
                                if let Some(bus) = app.buses.get(bus_idx) {
                                    let mut set = selected_steps.lock().unwrap();
                                    set.clear();
                                    set.extend(0..bus.gate_sequence.num_steps);
                                    drop(set);
                                    editor.runtime_mut().set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "delete-selected-bus-steps" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            if let Some(bus_idx) = bus_idx {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    let steps: Vec<usize> = {
                                        let mut set = selected_steps.lock().unwrap();
                                        let mut steps: Vec<usize> = set.iter().copied().collect();
                                        steps.sort_unstable();
                                        set.clear();
                                        steps
                                    };
                                    for step in steps {
                                        if step >= bus.gate_sequence.num_steps {
                                            continue;
                                        }
                                        bus.gate_sequence.steps[step] = false;
                                        bus.gate_sequence.velocities[step] = 1.0;
                                        bus.gate_sequence.durations[step] = 1.0;
                                        bus.gate_sequence.syncs[step] = 0.0;
                                        bus.gate_sequence.timebase_plocks[step] = None;
                                        bus.gate_sequence.swing_plocks[step] = None;
                                        bus.gate_sequence.swing_resolution_plocks[step] = None;
                                        for slot in &mut bus.effect_slots {
                                            if let Some(step_plocks) = slot.plocks.get_mut(step) {
                                                for value in step_plocks {
                                                    *value = None;
                                                }
                                            }
                                        }
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "move-bus-step-drag" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let start = map_number(map, "start").map(|value| value as usize);
                            let target = map_number(map, "target").map(|value| value as usize);
                            if let (Some(bus_idx), Some(start), Some(target)) =
                                (bus_idx, start, target)
                            {
                                if start != target {
                                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                                        let num_steps = bus.gate_sequence.num_steps;
                                        if start < num_steps && target < num_steps {
                                            let delta = target as isize - start as isize;
                                            let mut move_selection = false;
                                            let steps: Vec<usize> = {
                                                let set = selected_steps.lock().unwrap();
                                                if set.contains(&start) {
                                                    move_selection = true;
                                                    let mut steps: Vec<usize> =
                                                        set.iter().copied().collect();
                                                    steps.sort_unstable();
                                                    steps
                                                } else {
                                                    vec![start]
                                                }
                                            };
                                            if let (Some(&first), Some(&last)) =
                                                (steps.first(), steps.last())
                                            {
                                                let new_first = first as isize + delta;
                                                let new_last = last as isize + delta;
                                                if new_first >= 0 && new_last < num_steps as isize {
                                                    let snapshots: Vec<_> = steps
                                                        .iter()
                                                        .map(|&step| {
                                                            (
                                                                step,
                                                                bus.gate_sequence.steps[step],
                                                                bus.gate_sequence.velocities[step],
                                                                bus.gate_sequence.durations[step],
                                                                bus.gate_sequence.syncs[step],
                                                                bus.gate_sequence.timebase_plocks
                                                                    [step],
                                                                bus.gate_sequence.swing_plocks
                                                                    [step],
                                                                bus.gate_sequence
                                                                    .swing_resolution_plocks[step],
                                                                bus.effect_slots
                                                                    .iter()
                                                                    .map(|slot| {
                                                                        slot.plocks
                                                                            .get(step)
                                                                            .cloned()
                                                                            .unwrap_or_default()
                                                                    })
                                                                    .collect::<Vec<_>>(),
                                                            )
                                                        })
                                                        .collect();
                                                    for &step in &steps {
                                                        bus.gate_sequence.steps[step] = false;
                                                        bus.gate_sequence.velocities[step] = 1.0;
                                                        bus.gate_sequence.durations[step] = 1.0;
                                                        bus.gate_sequence.syncs[step] = 0.0;
                                                        bus.gate_sequence.timebase_plocks[step] =
                                                            None;
                                                        bus.gate_sequence.swing_plocks[step] = None;
                                                        bus.gate_sequence.swing_resolution_plocks
                                                            [step] = None;
                                                        for slot in &mut bus.effect_slots {
                                                            if let Some(step_plocks) =
                                                                slot.plocks.get_mut(step)
                                                            {
                                                                for value in step_plocks {
                                                                    *value = None;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    let moved_steps: Vec<usize> = snapshots
                                                        .iter()
                                                        .map(|(step, ..)| {
                                                            (*step as isize + delta) as usize
                                                        })
                                                        .collect();
                                                    for (snapshot, dst_step) in snapshots
                                                        .iter()
                                                        .zip(moved_steps.iter().copied())
                                                    {
                                                        bus.gate_sequence.steps[dst_step] =
                                                            snapshot.1;
                                                        bus.gate_sequence.velocities[dst_step] =
                                                            snapshot.2;
                                                        bus.gate_sequence.durations[dst_step] =
                                                            snapshot.3;
                                                        bus.gate_sequence.syncs[dst_step] =
                                                            snapshot.4;
                                                        bus.gate_sequence.timebase_plocks
                                                            [dst_step] = snapshot.5;
                                                        bus.gate_sequence.swing_plocks[dst_step] =
                                                            snapshot.6;
                                                        bus.gate_sequence.swing_resolution_plocks
                                                            [dst_step] = snapshot.7;
                                                        for (slot_idx, slot_plocks) in
                                                            snapshot.8.iter().enumerate()
                                                        {
                                                            let Some(slot) =
                                                                bus.effect_slots.get_mut(slot_idx)
                                                            else {
                                                                continue;
                                                            };
                                                            let Some(dst_plocks) =
                                                                slot.plocks.get_mut(dst_step)
                                                            else {
                                                                continue;
                                                            };
                                                            for (param_idx, value) in
                                                                slot_plocks.iter().enumerate()
                                                            {
                                                                if param_idx < dst_plocks.len() {
                                                                    dst_plocks[param_idx] = *value;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if move_selection {
                                                        let mut set =
                                                            selected_steps.lock().unwrap();
                                                        set.clear();
                                                        set.extend(moved_steps);
                                                    }
                                                    app.publish_bus_gate_runtime();
                                                    let rt = editor.runtime_mut();
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "selected-steps",
                                                        build_selection_value(&selected_steps),
                                                    );
                                                    sync_bus_mixer_state(rt, &app);
                                                    rt.run_reactive_cycle();
                                                    editor.refresh_runtime_side_effects();
                                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "shift-selected-bus-steps" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let direction =
                                map_number(map, "direction").map(|value| value.signum() as isize);
                            if let (Some(bus_idx), Some(delta)) = (bus_idx, direction) {
                                if delta != 0 {
                                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                                        let steps: Vec<usize> = {
                                            let set = selected_steps.lock().unwrap();
                                            let mut steps: Vec<usize> =
                                                set.iter().copied().collect();
                                            steps.sort_unstable();
                                            steps
                                        };
                                        if let (Some(&first), Some(&last)) =
                                            (steps.first(), steps.last())
                                        {
                                            let num_steps = bus.gate_sequence.num_steps;
                                            let can_shift = if delta < 0 {
                                                first > 0
                                            } else {
                                                last + 1 < num_steps
                                            };
                                            if can_shift {
                                                let snapshots: Vec<_> = steps
                                                    .iter()
                                                    .map(|&step| {
                                                        (
                                                            step,
                                                            bus.gate_sequence.steps[step],
                                                            bus.gate_sequence.velocities[step],
                                                            bus.gate_sequence.durations[step],
                                                            bus.gate_sequence.syncs[step],
                                                            bus.gate_sequence.timebase_plocks[step],
                                                            bus.gate_sequence.swing_plocks[step],
                                                            bus.gate_sequence
                                                                .swing_resolution_plocks[step],
                                                            bus.effect_slots
                                                                .iter()
                                                                .map(|slot| {
                                                                    slot.plocks
                                                                        .get(step)
                                                                        .cloned()
                                                                        .unwrap_or_default()
                                                                })
                                                                .collect::<Vec<_>>(),
                                                        )
                                                    })
                                                    .collect();
                                                for &step in &steps {
                                                    bus.gate_sequence.steps[step] = false;
                                                    bus.gate_sequence.velocities[step] = 1.0;
                                                    bus.gate_sequence.durations[step] = 1.0;
                                                    bus.gate_sequence.syncs[step] = 0.0;
                                                    bus.gate_sequence.timebase_plocks[step] = None;
                                                    bus.gate_sequence.swing_plocks[step] = None;
                                                    bus.gate_sequence.swing_resolution_plocks
                                                        [step] = None;
                                                    for slot in &mut bus.effect_slots {
                                                        if let Some(step_plocks) =
                                                            slot.plocks.get_mut(step)
                                                        {
                                                            for value in step_plocks {
                                                                *value = None;
                                                            }
                                                        }
                                                    }
                                                }
                                                let shifted_steps: Vec<usize> = snapshots
                                                    .iter()
                                                    .map(|(step, ..)| {
                                                        (*step as isize + delta) as usize
                                                    })
                                                    .collect();
                                                for (snapshot, dst_step) in snapshots
                                                    .iter()
                                                    .zip(shifted_steps.iter().copied())
                                                {
                                                    bus.gate_sequence.steps[dst_step] = snapshot.1;
                                                    bus.gate_sequence.velocities[dst_step] =
                                                        snapshot.2;
                                                    bus.gate_sequence.durations[dst_step] =
                                                        snapshot.3;
                                                    bus.gate_sequence.syncs[dst_step] = snapshot.4;
                                                    bus.gate_sequence.timebase_plocks[dst_step] =
                                                        snapshot.5;
                                                    bus.gate_sequence.swing_plocks[dst_step] =
                                                        snapshot.6;
                                                    bus.gate_sequence.swing_resolution_plocks
                                                        [dst_step] = snapshot.7;
                                                    for (slot_idx, slot_plocks) in
                                                        snapshot.8.iter().enumerate()
                                                    {
                                                        let Some(slot) =
                                                            bus.effect_slots.get_mut(slot_idx)
                                                        else {
                                                            continue;
                                                        };
                                                        let Some(dst_plocks) =
                                                            slot.plocks.get_mut(dst_step)
                                                        else {
                                                            continue;
                                                        };
                                                        for (param_idx, value) in
                                                            slot_plocks.iter().enumerate()
                                                        {
                                                            if param_idx < dst_plocks.len() {
                                                                dst_plocks[param_idx] = *value;
                                                            }
                                                        }
                                                    }
                                                }
                                                {
                                                    let mut set = selected_steps.lock().unwrap();
                                                    set.clear();
                                                    set.extend(shifted_steps);
                                                }
                                                app.publish_bus_gate_runtime();
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "selected-steps",
                                                    build_selection_value(&selected_steps),
                                                );
                                                sync_bus_mixer_state(rt, &app);
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-sequencer-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let param = map_string(map, "param");
                            let value = map_number(map, "value").map(|value| value as f32);
                            let label = map_string(map, "label");
                            if let (Some(bus_idx), Some(param)) = (bus_idx, param) {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    let selected_bus_steps: Vec<usize> = selected_steps
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .copied()
                                        .filter(|step| *step < bus.gate_sequence.num_steps)
                                        .collect();
                                    let write_plock =
                                        !selected_bus_steps.is_empty() && param != "num-steps";
                                    match param.as_str() {
                                        "num-steps" => {
                                            if let Some(value) = value {
                                                bus.gate_sequence.set_num_steps(value as usize);
                                            }
                                        }
                                        "swing" => {
                                            if let Some(value) = value {
                                                let swing = value.clamp(50.0, 75.0);
                                                if write_plock {
                                                    for step in &selected_bus_steps {
                                                        bus.gate_sequence.swing_plocks[*step] =
                                                            Some(swing);
                                                    }
                                                } else {
                                                    bus.gate_sequence.swing = swing;
                                                }
                                            }
                                        }
                                        "timebase" => {
                                            if let Some(label) = label {
                                                let normalized = label.to_ascii_lowercase();
                                                if let Some(idx) =
                                                    Timebase::LABELS.iter().position(|candidate| {
                                                        candidate.to_ascii_lowercase() == normalized
                                                    })
                                                {
                                                    let timebase = Timebase::ALL[idx];
                                                    if write_plock {
                                                        for step in &selected_bus_steps {
                                                            bus.gate_sequence.timebase_plocks
                                                                [*step] = Some(timebase);
                                                        }
                                                    } else {
                                                        bus.gate_sequence.timebase = timebase;
                                                    }
                                                }
                                            }
                                        }
                                        "swing-resolution" => {
                                            if let Some(label) = label {
                                                let normalized = label.to_ascii_lowercase();
                                                if let Some(idx) = SwingResolution::LABELS
                                                    .iter()
                                                    .position(|candidate| {
                                                        candidate.to_ascii_lowercase() == normalized
                                                    })
                                                {
                                                    let resolution = SwingResolution::ALL[idx];
                                                    if write_plock {
                                                        for step in &selected_bus_steps {
                                                            bus.gate_sequence
                                                                .swing_resolution_plocks[*step] =
                                                                Some(resolution);
                                                        }
                                                    } else {
                                                        bus.gate_sequence.swing_resolution =
                                                            resolution;
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-bus-effect-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(value)) =
                                (bus_idx, slot_idx, param_idx, value)
                            {
                                let desc = app
                                    .buses
                                    .get(bus_idx)
                                    .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                    .and_then(|desc| desc.params.get(param_idx))
                                    .cloned();
                                match app.apply_recorded_bus_effect_value_mutation(
                                    bus_idx,
                                    slot_idx,
                                    "Set bus effect parameter",
                                    format!("param:{param_idx}"),
                                    |app| app.set_bus_effect_param(
                                        bus_idx, slot_idx, param_idx, value,
                                    ),
                                ) {
                                    Ok(()) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        sync_bus_effect_param_value_field(
                                            editor.runtime_mut(),
                                            &app,
                                            bus_idx,
                                            slot_idx,
                                            param_idx,
                                        );
                                        if desc.as_ref().is_some_and(param_change_needs_fx_rebuild)
                                        {
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error setting bus effect param: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "set-bus-effect-plock" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(value)) =
                                (bus_idx, slot_idx, param_idx, value)
                            {
                                let steps: Vec<usize> =
                                    selected_steps.lock().unwrap().iter().copied().collect();
                                let result = app.apply_recorded_bus_effect_value_mutation(
                                    bus_idx,
                                    slot_idx,
                                    "Set bus effect p-lock",
                                    format!("plock:param:{param_idx}"),
                                    |app| {
                                        for step in steps {
                                            app.set_bus_effect_plock(
                                                bus_idx, slot_idx, step, param_idx, value,
                                            )?;
                                        }
                                        Ok(())
                                    },
                                );
                                match result {
                                    Ok(()) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error setting bus effect p-lock: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "set-bus-effect-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(label)) =
                                (bus_idx, slot_idx, param_idx, label)
                            {
                                if let Some(selected_idx) = app.bus_effect_param_option_index(
                                    bus_idx, slot_idx, param_idx, &label,
                                ) {
                                    let is_host_sidechain = matches!(
                                        app.buses
                                            .get(bus_idx)
                                            .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                            .and_then(|desc| desc.params.get(param_idx))
                                            .and_then(|param| param.host_control.as_ref()),
                                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                                    );
                                    match app.apply_recorded_bus_effect_value_mutation(
                                        bus_idx,
                                        slot_idx,
                                        "Set bus effect option",
                                        format!("param:{param_idx}"),
                                        |app| {
                                            if is_host_sidechain {
                                                app.apply_bus_effect_sidechain_selection(
                                                    bus_idx, slot_idx, param_idx, selected_idx,
                                                );
                                            }
                                            app.set_bus_effect_param(
                                                bus_idx, slot_idx, param_idx, selected_idx as f32,
                                            )
                                        },
                                    ) {
                                        Ok(()) => {
                                            app.publish_bus_gate_runtime();
                                            *bus_state.lock().unwrap() = app.buses.clone();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.set_reactive(
                                                "SEQ",
                                                "bus-effects",
                                                build_bus_effects_value_for_selection(
                                                    &app,
                                                    Some(&selected_steps),
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error setting bus effect option: {error}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-effect-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(label)) =
                                (bus_idx, slot_idx, param_idx, label)
                            {
                                if let Some(selected_idx) = app.bus_effect_param_option_index(
                                    bus_idx, slot_idx, param_idx, &label,
                                ) {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    let result = app.apply_recorded_bus_effect_value_mutation(
                                        bus_idx,
                                        slot_idx,
                                        "Set bus effect p-lock option",
                                        format!("plock:param:{param_idx}"),
                                        |app| {
                                            for step in steps {
                                                app.set_bus_effect_plock(
                                                    bus_idx,
                                                    slot_idx,
                                                    step,
                                                    param_idx,
                                                    selected_idx as f32,
                                                )?;
                                            }
                                            Ok(())
                                        },
                                    );
                                    match result {
                                        Ok(()) => {
                                            app.publish_bus_gate_runtime();
                                            *bus_state.lock().unwrap() = app.buses.clone();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Error setting bus effect p-lock option: {error}"
                                            )))
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-effect-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(label)) =
                                (slot_idx, param_idx, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let selected_idx = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|d| d.get(slot_idx))
                                    .and_then(|d| d.params.get(param_idx))
                                    .and_then(|p| match &p.kind {
                                        sequencer::effects::ParamKind::Enum { labels } => {
                                            labels.iter().position(|item| item == &label)
                                        }
                                        _ => None,
                                    })
                                    .or_else(|| {
                                        let is_delay_time = app
                                            .graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .map(|d| d.name == "Delay")
                                            .unwrap_or(false)
                                            && param_idx == 2;
                                        is_delay_time.then(|| {
                                            sequencer::effects::SyncDivision::ALL
                                                .iter()
                                                .position(|div| div.label() == label)
                                        })?
                                    });
                                if let Some(selected_idx) = selected_idx {
                                    let is_host_sidechain = matches!(
                                        app.graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .and_then(|d| d.params.get(param_idx))
                                            .and_then(|p| p.host_control.as_ref()),
                                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                                    );
                                    if is_host_sidechain {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetEffectParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    } else {
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock, neural_history_before) =
                                            record_selected_neural_effect_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                slot_idx,
                                                param_idx,
                                            value,
                                        );
                                        if let Some(before) = neural_history_before {
                                            app.commit_applied_scene_structure_mutation(
                                                before,
                                                "Edit neural override",
                                            );
                                        }
                                        if !wrote_neural_plock {
                                            let steps: Vec<usize> = selected_steps
                                                .lock()
                                                .unwrap()
                                                .iter()
                                                .copied()
                                                .collect();
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetEffectPlockMulti {
                                                    track,
                                                    slot_idx,
                                                    steps,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                        sync_effect_param_authoring_display(
                                            &mut editor,
                                            EffectParamDisplaySync {
                                                state: &state,
                                                effect_descriptors: &app.graph.effect_descriptors,
                                                app: &app,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-midi-fx-param" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(value)) =
                                (slot_idx, param_idx, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                let desc = chain
                                    .get(slot_idx)
                                    .and_then(|name| {
                                        sequencer::lisp_host::load_midi_fx_descriptor(name)
                                    })
                                    .and_then(|desc| desc.params.get(param_idx).cloned());
                                let clamped = desc
                                    .as_ref()
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                if let Some(_slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetMidiFxParam {
                                            track,
                                            slot_idx,
                                            param_idx,
                                            value: clamped,
                                        },
                                    );
                                    sync_midi_fx_param_value_field(
                                        editor.runtime_mut(),
                                        &state,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        None,
                                    );
                                    if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-midi-fx-plock" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(value)) =
                                (slot_idx, param_idx, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                let clamped = chain
                                    .get(slot_idx)
                                    .and_then(|name| {
                                        sequencer::lisp_host::load_midi_fx_descriptor(name)
                                    })
                                    .and_then(|desc| desc.params.get(param_idx).cloned())
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                if let Some(_slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetMidiFxPlockMulti {
                                            track,
                                            steps,
                                            slot_idx,
                                            param_idx,
                                            value: clamped,
                                        },
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-midi-fx-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(label)) =
                                (slot_idx, param_idx, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                if let Some(selected_idx) = chain
                                    .get(slot_idx)
                                    .and_then(|name| midi_fx_option_index(name, param_idx, &label))
                                {
                                    if let Some(_slot) = state
                                        .pattern
                                        .midi_fx_slots
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                    {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetMidiFxParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-midi-fx-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(label)) =
                                (slot_idx, param_idx, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                if let Some(selected_idx) = chain
                                    .get(slot_idx)
                                    .and_then(|name| midi_fx_option_index(name, param_idx, &label))
                                {
                                    if let Some(_slot) = state
                                        .pattern
                                        .midi_fx_slots
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                    {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetMidiFxPlockMulti {
                                                track,
                                                steps,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-base-note" => {
                        if let Value::Map(ref map) = payload {
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let Some(value) = value {
                                let track = current_track.load(Ordering::Relaxed);
                                let clamped = value.clamp(-48.0, 48.0);
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetInstrumentBaseNoteOffset {
                                        track,
                                        value: clamped,
                                    },
                                );
                                sync_instrument_base_note_value_field(
                                    editor.runtime_mut(),
                                    &app,
                                    track,
                                );
                            }
                        }
                    }
                    "set-track-plock-entry" => {
                        if let Value::Map(ref map) = payload {
                            let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let rack_slot =
                                map.get("rack-slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(target), Some(step), Some(value)) = (target, step, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                match target.as_str() {
                                    "timebase" => {
                                        let idx = (value.round() as usize)
                                            .min(sequencer::sequencer::Timebase::ALL.len() - 1);
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetTimebasePlock {
                                                track,
                                                step,
                                                timebase: Some(
                                                    sequencer::sequencer::Timebase::ALL[idx],
                                                ),
                                            },
                                        );
                                    }
                                    "swing" => {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetTrackSwingPlock {
                                                track,
                                                step,
                                                value: Some(value),
                                            },
                                        );
                                    }
                                    "swing-resolution" => {
                                        let idx = (value.round() as usize).min(
                                            sequencer::sequencer::SwingResolution::ALL.len() - 1,
                                        );
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetTrackSwingResolutionPlock {
                                                track,
                                                step,
                                                resolution: Some(
                                                    sequencer::sequencer::SwingResolution::ALL[idx],
                                                ),
                                            },
                                        );
                                    }
                                    "step-param" => {
                                        if let Some(param_idx) = param_idx {
                                            if let Some(param) =
                                                sequencer::sequencer::StepParam::ALL.get(param_idx)
                                            {
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetStepParam {
                                                        track,
                                                        step,
                                                        param: *param,
                                                        value,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    "instrument" => {
                                        if let Some(param_idx) = param_idx {
                                            if let Some(desc) = app
                                                .graph
                                                .instrument_descriptors
                                                .get(track)
                                                .and_then(|d| d.params.get(param_idx))
                                                .cloned()
                                            {
                                                let stored =
                                                    desc.clamp(desc.user_input_to_stored(value));
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetInstrumentPlock {
                                                        track,
                                                        step,
                                                        param_idx,
                                                        value: stored,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(_slot) = state
                                                .pattern
                                                .effect_chains
                                                .get(track)
                                                .and_then(|chain| chain.get(slot_idx))
                                            {
                                                let clamped = app
                                                    .graph
                                                    .effect_descriptors
                                                    .get(track)
                                                    .and_then(|d| d.get(slot_idx))
                                                    .and_then(|d| d.params.get(param_idx))
                                                    .map(|p| value.clamp(p.min, p.max))
                                                    .unwrap_or(value);
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetEffectPlock {
                                                        track,
                                                        step,
                                                        slot_idx,
                                                        param_idx,
                                                        value: clamped,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    "rack-macro" => {
                                        if let Some(param_idx) = param_idx {
                                            if let Some(id) =
                                                sequencer::sequencer::RackMacroId::from_index(
                                                    param_idx,
                                                )
                                            {
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetRackMacroPlockMulti {
                                                        track,
                                                        steps: vec![step],
                                                        macro_idx: id.index(),
                                                        value,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    "rack-effect" => {
                                        if let (
                                            Some(rack_slot),
                                            Some(effect_slot),
                                            Some(param_idx),
                                        ) = (rack_slot, slot_idx, param_idx)
                                        {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetRackSlotEffectPlockMulti {
                                                    track,
                                                    steps: vec![step],
                                                    rack_slot_idx: rack_slot,
                                                    effect_slot_idx: effect_slot,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(_slot) = state
                                                .pattern
                                                .midi_fx_slots
                                                .get(track)
                                                .and_then(|slots| slots.get(slot_idx))
                                            {
                                                let chain = state.pattern.track_params[track]
                                                    .midi_fx_chain();
                                                let clamped = chain
                                                    .get(slot_idx)
                                                    .and_then(|name| {
                                                        sequencer::lisp_host::load_midi_fx_descriptor(name)
                                                    })
                                                    .and_then(|desc| {
                                                        desc.params.get(param_idx).cloned()
                                                    })
                                                    .map(|p| value.clamp(p.min, p.max))
                                                    .unwrap_or(value);
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetMidiFxPlockMulti {
                                                        track,
                                                        steps: vec![step],
                                                        slot_idx,
                                                        param_idx,
                                                        value: clamped,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "set-track-plock-entry-option" => {
                        if let Value::Map(ref map) = payload {
                            let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let rack_slot =
                                map.get("rack-slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(target), Some(step), Some(label)) = (target, step, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                match target.as_str() {
                                    "timebase" => {
                                        if let Some(idx) = sequencer::sequencer::Timebase::LABELS
                                            .iter()
                                            .position(|item| *item == label)
                                        {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetTimebasePlock {
                                                    track,
                                                    step,
                                                    timebase: Some(
                                                        sequencer::sequencer::Timebase::ALL[idx],
                                                    ),
                                                },
                                            );
                                        }
                                    }
                                    "swing-resolution" => {
                                        if let Some(idx) =
                                            sequencer::sequencer::SwingResolution::LABELS
                                                .iter()
                                                .position(|item| *item == label)
                                        {
                                            app::apply_command(
                                                &mut app,
                                                app::AppCommand::SetTrackSwingResolutionPlock {
                                                    track,
                                                    step,
                                                    resolution: Some(
                                                        sequencer::sequencer::SwingResolution::ALL[idx],
                                                    ),
                                                },
                                            );
                                        }
                                    }
                                    "step-param" => {}
                                    "instrument" => {
                                        if let Some(param_idx) = param_idx {
                                            if let Some(selected_idx) = app
                                                .graph
                                                .instrument_descriptors
                                                .get(track)
                                                .and_then(|d| d.params.get(param_idx))
                                                .and_then(|p| match &p.kind {
                                                    sequencer::effects::ParamKind::Enum {
                                                        labels,
                                                    } => labels
                                                        .iter()
                                                        .position(|item| item == &label),
                                                    sequencer::effects::ParamKind::Boolean => {
                                                        match label.as_str() {
                                                            "on" | "ON" => Some(1),
                                                            "off" | "OFF" => Some(0),
                                                            _ => None,
                                                        }
                                                    }
                                                    _ => None,
                                                })
                                            {
                                                app::apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetInstrumentPlock {
                                                        track,
                                                        step,
                                                        param_idx,
                                                        value: selected_idx as f32,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(selected_idx) = app
                                                .graph
                                                .effect_descriptors
                                                .get(track)
                                                .and_then(|d| d.get(slot_idx))
                                                .and_then(|d| d.params.get(param_idx))
                                                .and_then(|p| match &p.kind {
                                                    sequencer::effects::ParamKind::Enum {
                                                        labels,
                                                    } => labels
                                                        .iter()
                                                        .position(|item| item == &label),
                                                    sequencer::effects::ParamKind::Boolean => {
                                                        match label.as_str() {
                                                            "on" | "ON" => Some(1),
                                                            "off" | "OFF" => Some(0),
                                                            _ => None,
                                                        }
                                                    }
                                                    _ => None,
                                                })
                                            {
                                                if let Some(_slot) = state
                                                    .pattern
                                                    .effect_chains
                                                    .get(track)
                                                    .and_then(|chain| chain.get(slot_idx))
                                                {
                                                    app::apply_command(
                                                        &mut app,
                                                        app::AppCommand::SetEffectPlock {
                                                            track,
                                                            step,
                                                            slot_idx,
                                                            param_idx,
                                                            value: selected_idx as f32,
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    "rack-effect" => {
                                        if let (
                                            Some(rack_slot),
                                            Some(effect_slot),
                                            Some(param_idx),
                                        ) = (rack_slot, slot_idx, param_idx)
                                        {
                                            match app.rack_slot_effect_option_value(
                                                track,
                                                rack_slot,
                                                effect_slot,
                                                param_idx,
                                                &label,
                                            ) {
                                                Ok(value) => {
                                                    app::apply_command(
                                                        &mut app,
                                                        app::AppCommand::SetRackSlotEffectPlockMulti {
                                                            track,
                                                            steps: vec![step],
                                                            rack_slot_idx: rack_slot,
                                                            effect_slot_idx: effect_slot,
                                                            param_idx,
                                                            value,
                                                        },
                                                    );
                                                }
                                                Err(error) => editor.handle_host_event(
                                                    HostEvent::Status(format!(
                                                        "Error editing rack-slot effect lock: {error}"
                                                    )),
                                                ),
                                            }
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            let chain =
                                                state.pattern.track_params[track].midi_fx_chain();
                                            if let Some(selected_idx) = chain
                                                .get(slot_idx)
                                                .and_then(|name| {
                                                    sequencer::lisp_host::load_midi_fx_descriptor(
                                                        name,
                                                    )
                                                })
                                                .and_then(|desc| {
                                                    desc.params.get(param_idx).and_then(|p| {
                                                        match &p.kind {
                                                            sequencer::effects::ParamKind::Enum {
                                                                labels,
                                                            } => labels
                                                                .iter()
                                                                .position(|item| item == &label),
                                                            sequencer::effects::ParamKind::Boolean => {
                                                                match label.as_str() {
                                                                    "on" | "ON" => Some(1),
                                                                    "off" | "OFF" => Some(0),
                                                                    _ => None,
                                                                }
                                                            }
                                                            _ => None,
                                                        }
                                                    })
                                                })
                                            {
                                                if let Some(_slot) = state
                                                    .pattern
                                                    .midi_fx_slots
                                                    .get(track)
                                                    .and_then(|slots| slots.get(slot_idx))
                                                {
                                                    app::apply_command(
                                                        &mut app,
                                                        app::AppCommand::SetMidiFxPlockMulti {
                                                            track,
                                                            steps: vec![step],
                                                            slot_idx,
                                                            param_idx,
                                                            value: selected_idx as f32,
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "clear-track-plock-entry" => {
                        if let Value::Map(ref map) = payload {
                            let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let rack_slot =
                                map.get("rack-slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let target_track =
                                map.get("target-track")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) => Some(*n as usize),
                                        _ => None,
                                    });
                            let network_id =
                                map.get("network-id")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) => Some(*n as u64),
                                        _ => None,
                                    });
                            let neuron_idx =
                                map.get("neuron-idx")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) => Some(*n as usize),
                                        _ => None,
                                    });
                            if let Some(target) = target {
                                let track = current_track.load(Ordering::Relaxed);
                                let mut changed = false;
                                match target.as_str() {
                                    "timebase" => {
                                        if let Some(step) = step {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::SetTimebasePlock {
                                                    track,
                                                    step,
                                                    timebase: None,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "swing" => {
                                        if let Some(step) = step {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::SetTrackSwingPlock {
                                                    track,
                                                    step,
                                                    value: None,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "swing-resolution" => {
                                        if let Some(step) = step {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::SetTrackSwingResolutionPlock {
                                                    track,
                                                    step,
                                                    resolution: None,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "step-param" => {
                                        if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                            if let Some(param) =
                                                sequencer::sequencer::StepParam::ALL.get(param_idx)
                                            {
                                                changed = app::try_apply_command(
                                                    &mut app,
                                                    app::AppCommand::SetStepParam {
                                                        track,
                                                        step,
                                                        param: *param,
                                                        value: param.default_value(),
                                                    },
                                                )
                                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                            }
                                        }
                                    }
                                    "instrument" => {
                                        if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::ClearInstrumentPlockMulti {
                                                    track,
                                                    steps: vec![step],
                                                    param_idx,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(step), Some(slot_idx), Some(param_idx)) =
                                            (step, slot_idx, param_idx)
                                        {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::ClearEffectPlockMulti {
                                                    track,
                                                    steps: vec![step],
                                                    slot_idx,
                                                    param_idx,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "rack-macro" => {
                                        if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                            if let Some(id) =
                                                sequencer::sequencer::RackMacroId::from_index(
                                                    param_idx,
                                                )
                                            {
                                                changed = app::try_apply_command(
                                                    &mut app,
                                                    app::AppCommand::ClearRackMacroPlockMulti {
                                                        track,
                                                        steps: vec![step],
                                                        macro_idx: id.index(),
                                                    },
                                                )
                                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                            }
                                        }
                                    }
                                    "rack-effect" => {
                                        if let (
                                            Some(step),
                                            Some(rack_slot),
                                            Some(effect_slot),
                                            Some(param_idx),
                                        ) = (step, rack_slot, slot_idx, param_idx)
                                        {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::ClearRackSlotEffectPlockMulti {
                                                    track,
                                                    steps: vec![step],
                                                    rack_slot_idx: rack_slot,
                                                    effect_slot_idx: effect_slot,
                                                    param_idx,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(step), Some(slot_idx), Some(param_idx)) =
                                            (step, slot_idx, param_idx)
                                        {
                                            changed = app::try_apply_command(
                                                &mut app,
                                                app::AppCommand::ClearMidiFxPlockMulti {
                                                    track,
                                                    steps: vec![step],
                                                    slot_idx,
                                                    param_idx,
                                                },
                                            )
                                            .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                        }
                                    }
                                    "neural-instrument" => {
                                        if let (
                                            Some(network_id),
                                            Some(neuron_idx),
                                            Some(target_track),
                                            Some(param_idx),
                                        ) = (network_id, neuron_idx, target_track, param_idx)
                                        {
                                            let history_before = state.capture_project_scenes();
                                            match sequencer::lisp_host::clear_neural_instrument_plock_by_network_id(
                                                &state,
                                                network_id,
                                                neuron_idx,
                                                target_track,
                                                param_idx,
                                            ) {
                                                Ok(removed) => {
                                                    changed |= removed;
                                                    if removed {
                                                        app.commit_applied_scene_structure_mutation(
                                                            history_before,
                                                            "Clear neural override",
                                                        );
                                                    }
                                                }
                                                Err(error) => editor.handle_host_event(
                                                    HostEvent::Status(format!(
                                                        "Error clearing neuron instrument p-lock: {error}"
                                                    )),
                                                ),
                                            }
                                        }
                                    }
                                    "neural-effect" => {
                                        if let (
                                            Some(network_id),
                                            Some(neuron_idx),
                                            Some(target_track),
                                            Some(slot_idx),
                                            Some(param_idx),
                                        ) = (
                                            network_id,
                                            neuron_idx,
                                            target_track,
                                            slot_idx,
                                            param_idx,
                                        ) {
                                            let history_before = state.capture_project_scenes();
                                            match sequencer::lisp_host::clear_neural_effect_plock_by_network_id(
                                                &state,
                                                network_id,
                                                neuron_idx,
                                                target_track,
                                                slot_idx,
                                                param_idx,
                                            ) {
                                                Ok(removed) => {
                                                    changed |= removed;
                                                    if removed {
                                                        app.commit_applied_scene_structure_mutation(
                                                            history_before,
                                                            "Clear neural override",
                                                        );
                                                    }
                                                }
                                                Err(error) => editor.handle_host_event(
                                                    HostEvent::Status(format!(
                                                        "Error clearing neuron effect p-lock: {error}"
                                                    )),
                                                ),
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                if changed {
                                    let selection = selected_neural_neurons.lock().unwrap().clone();
                                    sync_track_plocks_for_neural_selection(
                                        editor.runtime_mut(),
                                        &app,
                                        &state,
                                        track,
                                        &selected_steps,
                                        &selection,
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.mark_needs_redraw();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                } else if step.is_some() {
                                    state.publish_scheduler_snapshot();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "preview-plock-variant" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(label) = map_string(map, "label") {
                                let track = current_track.load(Ordering::Relaxed);
                                if !selected_steps.lock().unwrap().is_empty() {
                                    preview_plock_variant = None;
                                    continue;
                                }
                                preview_plock_variant = Some((track, label));
                                {
                                    let rt = editor.runtime_mut();
                                    sync_track_plock_variant_preview(
                                        rt,
                                        &app,
                                        &state,
                                        track,
                                        &selected_steps,
                                        preview_plock_variant.as_ref(),
                                    );
                                    rt.run_reactive_cycle();
                                }
                                editor.refresh_runtime_side_effects();
                                editor.refresh_visible_layouts_for_buffer_named("*step*");
                                editor.mark_needs_redraw();
                            }
                        }
                    }
                    "stamp-plock-variant" | "clear-step-variant-locks" => {
                        if let Value::Map(ref map) = payload {
                            preview_plock_variant = None;
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let fallback_step =
                                map.get("step").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let mut steps: Vec<usize> =
                                selected_steps.lock().unwrap().iter().copied().collect();
                            steps.sort_unstable();
                            if steps.is_empty() {
                                if let Some(step) = fallback_step {
                                    steps.push(step.min(MAX_STEPS - 1));
                                }
                            }
                            if steps.is_empty() {
                                continue;
                            }
                            let track = current_track.load(Ordering::Relaxed);
                            let is_clear = name == "clear-step-variant-locks"
                                || label.as_deref() == Some("def");
                            let assignment = label.as_ref().and_then(|label| {
                                state
                                    .plock_variant_registry_snapshot(track)
                                    .assignment_for_label(label)
                                    .map(|assignment| assignment.key.clone())
                            });
                            let outcome = app::edit::apply_recorded_step_mutation(
                                &mut app,
                                track,
                                &steps,
                                if is_clear {
                                    "Clear step variant locks"
                                } else {
                                    "Stamp step variant"
                                },
                                |app| {
                                    if is_clear {
                                        app.state.clear_variant_locks_for_steps_no_publish(
                                            track,
                                            &steps,
                                        );
                                    } else if let Some(key) = &assignment {
                                        app.state.stamp_variant_key_to_steps_no_publish(
                                            track,
                                            key,
                                            &steps,
                                        );
                                    }
                                    Ok(())
                                },
                            );
                            let changed = match outcome {
                                Ok(app::edit::EditOutcome::Applied(_)) => true,
                                Ok(app::edit::EditOutcome::NoOp) => false,
                                Ok(app::edit::EditOutcome::AppliedUnrecorded) => {
                                    editor.handle_host_event(HostEvent::Error(
                                        "Variant edit was applied without history".to_string(),
                                    ));
                                    false
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Could not apply variant edit: {error:?}"
                                    )));
                                    false
                                }
                            };
                            if changed {
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "add-bus-effect" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let effect_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                });
                            if let (Some(bus_idx), Some(effect_name)) = (bus_idx, effect_name) {
                                match app.apply_recorded_bus_effect_chain_mutation(
                                    bus_idx,
                                    "Add bus effect",
                                    |app| app.add_bus_effect_sync(bus_idx, &effect_name),
                                ) {
                                    Ok(slot_idx) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        editor.reset_widget_scroll_for_buffer_named("*fx*");
                                        let fx_render_status =
                                            editor.runtime_mut().take_status_message();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        if let Some(status) = fx_render_status {
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "FX UI error after adding bus effect: {status}"
                                            )));
                                        } else {
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added bus effect '{}' to slot {}",
                                                effect_name,
                                                slot_idx + 1
                                            )));
                                        }
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error adding bus effect: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "add-builtin-bus-effect" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let effect_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                });
                            if let (Some(bus_idx), Some(effect_name)) = (bus_idx, effect_name) {
                                match app.apply_recorded_bus_effect_chain_mutation(
                                    bus_idx,
                                    "Add bus effect",
                                    |app| app.add_builtin_bus_effect_sync(bus_idx, &effect_name),
                                ) {
                                    Ok(slot_idx) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        editor.reset_widget_scroll_for_buffer_named("*fx*");
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Added built-in bus effect '{}' to slot {}",
                                            effect_name,
                                            slot_idx + 1
                                        )));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error adding built-in bus effect: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "insert-builtin-bus-effect-before-slot" => {
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(bus_idx), Some(slot), Some(effect_name)) =
                            (bus_idx, slot, effect_name)
                        {
                            match app.apply_recorded_bus_effect_chain_mutation(
                                bus_idx,
                                "Insert bus effect",
                                |app| app.insert_builtin_bus_effect_before_slot_sync(
                                    bus_idx,
                                    slot,
                                    &effect_name,
                                ),
                            ) {
                                Ok(slot_idx) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted built-in bus effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting built-in bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-bus-effect-before-slot" => {
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(bus_idx), Some(slot), Some(effect_name)) =
                            (bus_idx, slot, effect_name)
                        {
                            match app.apply_recorded_bus_effect_chain_mutation(
                                bus_idx,
                                "Insert bus effect",
                                |app| app.insert_bus_effect_before_slot_sync(
                                    bus_idx,
                                    slot,
                                    &effect_name,
                                ),
                            ) {
                                Ok(slot_idx) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted bus effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "add-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                    if let Some(slot_idx) = app.next_free_custom_slot() {
                                        app.start_effect_compile(&effect_name, slot_idx);
                                        editor.runtime_mut().set_reactive(
                                            "SEQ",
                                            "compiling",
                                            Value::Bool(true),
                                        );
                                    } else {
                                        editor.handle_host_event(HostEvent::Status(
                                            "No free effect slots available".to_string(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    "add-effect-to-track" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(effect_name)) = (track, effect_name) {
                            if track >= app.tracks.len() {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Track {} does not exist",
                                    track + 1
                                )));
                                continue;
                            }
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            if let Some(slot_idx) = app.next_free_custom_slot() {
                                app.start_effect_compile(&effect_name, slot_idx);
                                let rt = editor.runtime_mut();
                                set_current_track_reactive(rt, app.tracks.len(), track);
                                rt.set_reactive("SEQ", "compiling", Value::Bool(true));
                                sync_track_mixer_state(rt, &app, &state);
                                sync_sidebar_browser(rt, &app, track);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Adding effect '{}' to track {}",
                                    effect_name,
                                    track + 1
                                )));
                            } else {
                                editor.handle_host_event(HostEvent::Status(
                                    "No free effect slots available".to_string(),
                                ));
                            }
                        }
                    }
                    "add-builtin-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    let track = current_track.load(Ordering::Relaxed);
                                    app.ui.cursor_track = track;
                                    match app.apply_recorded_track_effect_chain_mutation(
                                        track,
                                        "Add audio effect",
                                        |app| app.add_builtin_effect_sync(track, &effect_name),
                                    ) {
                                        Ok(slot_idx) => {
                                            let rt = editor.runtime_mut();
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
                                            rt.set_reactive(
                                                "SEQ",
                                                "step-has-plocks",
                                                build_step_has_plocks(
                                                    &state,
                                                    track,
                                                    &app.graph.effect_descriptors,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            editor.reset_widget_scroll_for_buffer_named("*fx*");
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added built-in effect '{}' to slot {}",
                                                effect_name,
                                                slot_idx + 1
                                            )));
                                        }
                                        Err(error) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error adding built-in effect: {error}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "add-builtin-effect-to-track" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(effect_name)) = (track, effect_name) {
                            if track >= app.tracks.len() {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Track {} does not exist",
                                    track + 1
                                )));
                                continue;
                            }
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.apply_recorded_track_effect_chain_mutation(
                                track,
                                "Add audio effect",
                                |app| app.add_builtin_effect_sync(track, &effect_name),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.reset_widget_scroll_for_buffer_named("*fx*");
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Added built-in effect '{}' to track {} slot {}",
                                        effect_name,
                                        track + 1,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding built-in effect: {error}"
                                ))),
                            }
                        }
                    }
                    "add-midi-fx" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(fx_name) = &*cell.borrow() {
                                    let fx_name = fx_name.clone();
                                    let track = current_track.load(Ordering::Relaxed);
                                    match app.apply_recorded_track_midi_fx_chain_mutation(
                                        track,
                                        "Add MIDI FX",
                                        |app| app.add_midi_fx_to_track_sync(track, &fx_name),
                                    ) {
                                        Ok(slot_idx) => {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "midi-effects",
                                                build_midi_effects_value(
                                                    &state,
                                                    track,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "step-has-plocks",
                                                build_step_has_plocks(
                                                    &state,
                                                    track,
                                                    &app.graph.effect_descriptors,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added MIDI FX '{}' to slot {}",
                                                fx_name,
                                                slot_idx + 1
                                            )));
                                        }
                                        Err(e) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error adding MIDI FX: {e}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "add-midi-fx-to-track" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let fx_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(fx_name)) = (track, fx_name) {
                            if track >= app.tracks.len() {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Track {} does not exist",
                                    track + 1
                                )));
                                continue;
                            }
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.apply_recorded_track_midi_fx_chain_mutation(
                                track,
                                "Add MIDI FX",
                                |app| app.add_midi_fx_to_track_sync(track, &fx_name),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(&state, track, &selected_steps),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.reset_widget_scroll_for_buffer_named("*fx*");
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Added MIDI FX '{}' to track {} slot {}",
                                        fx_name,
                                        track + 1,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding MIDI FX: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-builtin-effect-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(slot), Some(effect_name)) =
                            (track, slot, effect_name)
                        {
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.apply_recorded_track_effect_chain_mutation(
                                track,
                                "Insert audio effect",
                                |app| app.insert_builtin_effect_before_slot_sync(
                                    track,
                                    slot,
                                    &effect_name,
                                ),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted built-in effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting built-in effect: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-effect-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(slot), Some(effect_name)) =
                            (track, slot, effect_name)
                        {
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.apply_recorded_track_effect_chain_mutation(
                                track,
                                "Insert audio effect",
                                |app| app.insert_saved_effect_before_slot_sync(
                                    track,
                                    slot,
                                    &effect_name,
                                ),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting effect: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-midi-fx-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let fx_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(slot), Some(fx_name)) = (track, slot, fx_name) {
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.apply_recorded_track_midi_fx_chain_mutation(
                                track,
                                "Insert MIDI FX",
                                |app| app.insert_midi_fx_before_slot_sync(track, slot, &fx_name),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(&state, track, &selected_steps),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted MIDI FX '{}' at slot {}",
                                        fx_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting MIDI FX: {error}"
                                ))),
                            }
                        }
                    }
                    "move-effect-slot" => {
                        let source_track = extract_usize_from_payload(&payload, "source-track");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let target_track = extract_usize_from_payload(&payload, "target-track");
                        let target_slot = extract_usize_from_payload(&payload, "target-slot");
                        if let (Some(source_track), Some(source_slot), Some(target_track)) =
                            (source_track, source_slot, target_track)
                        {
                            if source_track != target_track {
                                editor.handle_host_event(HostEvent::Status(
                                    "Move audio effects within the same track for now".to_string(),
                                ));
                                continue;
                            }
                            current_track.store(target_track, Ordering::Relaxed);
                            app.ui.cursor_track = target_track;
                            match app.apply_recorded_track_effect_chain_mutation(
                                target_track,
                                "Move audio effect",
                                |app| app.move_effect_slot_sync(
                                    target_track,
                                    source_slot,
                                    target_slot,
                                ),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), target_track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            target_track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            target_track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved effect to slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error moving effect: {error}"
                                ))),
                            }
                        }
                    }
                    "copy-effect-values-to-all-scenes" => {
                        let chain = extract_string_from_payload(&payload, "chain");
                        let track = extract_usize_from_payload(&payload, "track");
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let slot_idx = extract_usize_from_payload(&payload, "slot");
                        let updated = match (chain.as_deref(), track, bus_idx, slot_idx) {
                            (Some("audio"), Some(track), _, Some(slot_idx)) => state
                                .copy_current_effect_values_to_all_track_patterns(track, slot_idx),
                            (Some("midi"), Some(track), _, Some(slot_idx)) => state
                                .copy_current_midi_fx_values_to_all_track_patterns(track, slot_idx),
                            (Some("bus"), _, Some(bus_idx), Some(slot_idx)) => {
                                app.copy_bus_effect_values_to_all_scenes(bus_idx, slot_idx)
                            }
                            _ => 0,
                        };
                        if updated > 0 {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Copied effect values to {updated} patterns/scenes"
                            )));
                        } else {
                            editor.handle_host_event(HostEvent::Status(
                                "Could not copy effect values: invalid effect target".to_string(),
                            ));
                        }
                    }
                    "copy-instrument-values-to-all-scenes" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
                        let updated = match (track, rack_slot) {
                            (Some(track), Some(rack_slot)) => state
                                .copy_current_rack_slot_instrument_values_to_all_track_patterns(
                                    track, rack_slot,
                                ),
                            (Some(track), None) => {
                                state.copy_current_instrument_values_to_all_track_patterns(track)
                            }
                            _ => 0,
                        };
                        if updated > 0 {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Copied instrument values to {updated} patterns/scenes"
                            )));
                        } else {
                            editor.handle_host_event(HostEvent::Status(
                                "Could not copy instrument values: invalid instrument target"
                                    .to_string(),
                            ));
                        }
                    }
                    "move-midi-fx-slot" => {
                        let source_track = extract_usize_from_payload(&payload, "source-track");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let target_track = extract_usize_from_payload(&payload, "target-track");
                        let target_slot = extract_usize_from_payload(&payload, "target-slot");
                        if let (Some(source_track), Some(source_slot), Some(target_track)) =
                            (source_track, source_slot, target_track)
                        {
                            if source_track != target_track {
                                editor.handle_host_event(HostEvent::Status(
                                    "Move MIDI effects within the same track for now".to_string(),
                                ));
                                continue;
                            }
                            current_track.store(target_track, Ordering::Relaxed);
                            app.ui.cursor_track = target_track;
                            match app.apply_recorded_track_midi_fx_chain_mutation(
                                target_track,
                                "Move MIDI FX",
                                |app| app.move_midi_fx_slot_sync(
                                    target_track,
                                    source_slot,
                                    target_slot,
                                ),
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), target_track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(
                                            &state,
                                            target_track,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            target_track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved MIDI FX to slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error moving MIDI FX: {error}"
                                ))),
                            }
                        }
                    }
                    "move-bus-effect-slot" => {
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let target_slot = extract_usize_from_payload(&payload, "target-slot");
                        if let (Some(bus_idx), Some(source_slot)) = (bus_idx, source_slot) {
                            match app.apply_recorded_bus_effect_chain_mutation(
                                bus_idx,
                                "Move bus effect",
                                |app| app.move_bus_effect_slot_sync(bus_idx, source_slot, target_slot),
                            ) {
                                Ok(slot_idx) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved bus effect to slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error moving bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "delete-bus-effect" => {
                        let bus_idx = match &payload {
                            Value::Map(map) => {
                                map.get("bus").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            _ => None,
                        };
                        let slot_idx = match &payload {
                            Value::Map(map) => {
                                map.get("slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            _ => None,
                        };
                        if let (Some(bus_idx), Some(slot_idx)) = (bus_idx, slot_idx) {
                            match app.apply_recorded_bus_effect_chain_mutation(
                                bus_idx,
                                "Delete bus effect",
                                |app| app.delete_bus_effect_slot(bus_idx, slot_idx),
                            ) {
                                Ok(()) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Deleted bus effect slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "delete-effect" => {
                        let slot_idx = match &payload {
                            Value::Map(map) => {
                                map.get("slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        };
                        let Some(slot_idx) = slot_idx else {
                            editor.handle_host_event(HostEvent::Status(
                                "No effect selected".to_string(),
                            ));
                            continue;
                        };
                        let track = current_track.load(Ordering::Relaxed);
                        match app.apply_recorded_track_effect_chain_mutation(
                            track,
                            "Delete audio effect",
                            |app| app.graph_controller().delete_custom_effect_slot(track, slot_idx),
                        ) {
                            Ok(()) => {
                                let rt = editor.runtime_mut();
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
                                rt.set_reactive(
                                    "SEQ",
                                    "midi-effects",
                                    build_midi_effects_value(&state, track, &selected_steps),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "step-has-plocks",
                                    build_step_has_plocks(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Deleted effect slot {}",
                                    slot_idx + 1
                                )));
                            }
                            Err(e) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting effect: {e}"
                                )));
                            }
                        }
                    }
                    "delete-midi-fx" => {
                        let slot_idx = match &payload {
                            Value::Map(map) => {
                                map.get("slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        };
                        let Some(slot_idx) = slot_idx else {
                            editor.handle_host_event(HostEvent::Status(
                                "No MIDI FX selected".to_string(),
                            ));
                            continue;
                        };
                        let track = current_track.load(Ordering::Relaxed);
                        match app.apply_recorded_track_midi_fx_chain_mutation(
                            track,
                            "Delete MIDI FX",
                            |app| app.delete_midi_fx_slot(track, slot_idx),
                        ) {
                            Ok(()) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "midi-effects",
                                    build_midi_effects_value(&state, track, &selected_steps),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "step-has-plocks",
                                    build_step_has_plocks(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Deleted MIDI FX slot {}",
                                    slot_idx + 1
                                )));
                            }
                            Err(e) => editor.handle_host_event(HostEvent::Status(format!(
                                "Error deleting MIDI FX: {e}"
                            ))),
                        }
                    }
                    "fork-track-pattern" => {
                        let track = match payload {
                            Value::Map(ref map) => map
                                .get("track")
                                .and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                                .unwrap_or_else(|| current_track.load(Ordering::Relaxed)),
                            Value::Number(n) => n as usize,
                            _ => current_track.load(Ordering::Relaxed),
                        };
                        let num_tracks = app.tracks.len();
                        if track >= num_tracks {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern fork failed: track {} is out of range",
                                track + 1
                            )));
                            continue;
                        }
                        let forked = app.apply_recorded_scene_structure_mutation(
                            "Fork track pattern",
                            |app| app.state.fork_current_track_pattern(
                                track,
                                num_tracks,
                                &app.graph.track_buffer_ids,
                                &app.graph.track_sample_rates,
                                &app.tracks,
                                &app.graph.track_instrument_types,
                            ).ok_or_else(|| format!("Could not fork track {} pattern", track + 1)),
                        );
                        let Ok(pattern_id) = forked else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern fork failed for track {}",
                                track + 1
                            )));
                            continue;
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Forked track {} pattern {}",
                            track + 1,
                            pattern_id.0
                        )));
                    }
                    "clone-track-pattern" => {
                        let (track, source_pattern_id) = match payload {
                            Value::Map(ref map) => (
                                map.get("track")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) if *n >= 0.0 => Some(*n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed)),
                                map.get("pattern-id")
                                    .or_else(|| map.get("pattern_id"))
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) if *n >= 0.0 => Some(PatternId(*n as u64)),
                                        _ => None,
                                    }),
                            ),
                            Value::Number(n) => (n as usize, None),
                            _ => (current_track.load(Ordering::Relaxed), None),
                        };
                        let num_tracks = app.tracks.len();
                        if track >= num_tracks {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern clone failed: track {} is out of range",
                                track + 1
                            )));
                            continue;
                        }
                        let cloned = app.apply_recorded_scene_structure_mutation(
                            "Clone track pattern",
                            |app| {
                                let cloned = if let Some(source_id) = source_pattern_id {
                                    app.state.clone_track_pattern_id_into_current_scene(
                                        track,
                                        source_id,
                                        num_tracks,
                                        &app.graph.track_buffer_ids,
                                        &app.graph.track_sample_rates,
                                        &app.tracks,
                                        &app.graph.track_instrument_types,
                                    )
                                } else {
                                    app.state.clone_current_scene_track_pattern(
                                        track,
                                        num_tracks,
                                        &app.graph.track_buffer_ids,
                                        &app.graph.track_sample_rates,
                                        &app.tracks,
                                        &app.graph.track_instrument_types,
                                    )
                                };
                                cloned.ok_or_else(|| format!(
                                    "Could not clone track {} pattern", track + 1
                                ))
                            },
                        );
                        let Ok(pattern_id) = cloned else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern clone failed for track {}",
                                track + 1
                            )));
                            continue;
                        };
                        let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
                        app.graph_controller().apply_sample_ids(&sample_ids);
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message = Some((
                                format!("Track pattern clone failed: {error}"),
                                Instant::now(),
                            ));
                        }
                        app.push_all_restored_defaults();
                        {
                            let mut guard = active_delete_target.lock().unwrap();
                            *guard = Some(ActiveDeleteTarget::TrackPattern { track, pattern_id });
                        }
                        active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cloned track {} pattern {}",
                            track + 1,
                            pattern_id.0
                        )));
                    }
                    "delete-track-pattern" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern delete failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let pattern_id = map
                            .get("pattern-id")
                            .or_else(|| map.get("pattern_id"))
                            .and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                                _ => None,
                            });
                        let (Some(track), Some(pattern_id)) = (track, pattern_id) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern delete failed: missing track or pattern id"
                                    .to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        let deleted = app.apply_recorded_scene_structure_mutation(
                            "Delete track pattern",
                            |app| {
                                if !app.state.delete_track_pattern(
                                    track,
                                    PatternId(pattern_id),
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.graph.track_sample_rates,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                ) {
                                    return Err(format!(
                                        "Could not delete track {} pattern {}",
                                        track + 1,
                                        pattern_id
                                    ));
                                }
                                // The live sample arrays must match the restored
                                // replacement pattern before the wrapper
                                // re-snapshots live state into it, or the old
                                // sample clobbers the pattern's sample_id.
                                let sample_ids =
                                    app.state.effective_pattern_sample_ids(num_tracks);
                                app.graph_controller().apply_sample_ids(&sample_ids);
                                Ok(())
                            },
                        );
                        if deleted.is_err() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern delete failed: track {}, pattern {}",
                                track + 1,
                                pattern_id
                            )));
                            continue;
                        }
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message = Some((
                                format!("Track pattern delete failed: {error}"),
                                Instant::now(),
                            ));
                        }
                        app.push_all_restored_defaults();
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Deleted track {} pattern {}",
                            track + 1,
                            pattern_id
                        )));
                    }
                    "set-scene-cell" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell share failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let pattern_id = map
                            .get("pattern-id")
                            .or_else(|| map.get("pattern_id"))
                            .and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                                _ => None,
                            });
                        let (Some(scene), Some(track), Some(pattern_id)) =
                            (scene, track, pattern_id)
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell share failed: missing scene, track, or pattern id"
                                    .to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        let shared = app.apply_recorded_scene_structure_mutation(
                            "Assign scene cell",
                            |app| {
                                if !app.state.set_scene_cell(
                                    scene,
                                    track,
                                    PatternId(pattern_id),
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.graph.track_sample_rates,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                ) {
                                    return Err(format!(
                                        "Could not assign scene {} track {}",
                                        scene + 1,
                                        track + 1
                                    ));
                                }
                                // The live sample arrays must match the restored
                                // pattern before the wrapper re-snapshots live
                                // state into it, or the old sample clobbers the
                                // pattern's sample_id.
                                let sample_ids =
                                    app.state.effective_pattern_sample_ids(num_tracks);
                                app.graph_controller().apply_sample_ids(&sample_ids);
                                Ok(())
                            },
                        );
                        if shared.is_err() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scene cell share failed: scene {}, track {}, pattern {}",
                                scene + 1,
                                track + 1,
                                pattern_id
                            )));
                            continue;
                        }
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message =
                                Some((format!("Scene cell share failed: {error}"), Instant::now()));
                        }
                        app.push_all_restored_defaults();
                        // Assigning into the current scene live-restores the
                        // pattern's params + sample; the generic pattern-epoch
                        // sync covers steps/params/mixer but not the fx and
                        // instrument-panel bindings, so refresh those here.
                        if scene == app.state.current_scene_index() {
                            if editor_has_visible_buffer(&editor, "*fx*") {
                                let ct = current_track_for_app(&mut app, &current_track)
                                    .unwrap_or(track);
                                let rt = editor.runtime_mut();
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
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                            } else {
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Shared track {} pattern {} into scene {}",
                            track + 1,
                            pattern_id,
                            scene + 1
                        )));
                    }
                    "clear-scene-cell" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell clear failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let (Some(scene), Some(track)) = (scene, track) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell clear failed: missing scene or track".to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        let cleared = app.apply_recorded_scene_structure_mutation(
                            "Clear scene cell",
                            |app| app.state.clear_scene_cell(
                                scene,
                                track,
                                num_tracks,
                                &app.graph.track_buffer_ids,
                                &app.graph.track_sample_rates,
                                &app.tracks,
                                &app.graph.track_instrument_types,
                            ).ok_or_else(|| format!(
                                "Could not clear scene {} track {}", scene + 1, track + 1
                            )),
                        );
                        let Ok(pattern_id) = cleared else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scene cell clear failed: scene {}, track {}",
                                scene + 1,
                                track + 1
                            )));
                            continue;
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cleared scene {} track {} pattern {}",
                            scene + 1,
                            track + 1,
                            pattern_id.0
                        )));
                    }
                    "launch-track-pattern" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern launch failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let pattern_id = map
                            .get("pattern-id")
                            .or_else(|| map.get("pattern_id"))
                            .and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                                _ => None,
                            });
                        let (Some(track), Some(pattern_id)) = (track, pattern_id) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern launch failed: missing track or pattern id"
                                    .to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        if track >= num_tracks {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern launch failed: track {} is out of range",
                                track + 1
                            )));
                            continue;
                        }
                        let launched = app.state.launch_track_pattern(
                            track,
                            PatternId(pattern_id),
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        );
                        if !launched {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern launch failed: pattern id {} is unavailable",
                                pattern_id
                            )));
                            continue;
                        }

                        let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
                        app.graph_controller().apply_sample_ids(&sample_ids);
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message = Some((
                                format!("Track pattern launch failed: {error}"),
                                Instant::now(),
                            ));
                        }
                        app.push_all_restored_defaults();

                        let ct = current_track_for_app(&mut app, &current_track).unwrap_or(track);
                        let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
                        let sequencer_visible = editor_has_visible_buffer(&editor, "*sequencer*");
                        let selected_neural_snapshot =
                            selected_neural_neurons.lock().unwrap().clone();
                        let rt = editor.runtime_mut();
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        sync_track_name_state(rt, &mut track_names, &app);
                        sync_pattern_state(rt, &state);
                        set_current_track_reactive(rt, app.tracks.len(), ct);
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                        sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
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
                        sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                        sync_step_param_lists(rt, &state, ct);
                        sync_track_mixer_state(rt, &app, &state);
                        sync_track_peak_fields(rt, &cached_track_peak_levels);
                        if fx_visible {
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
                        } else {
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                        }
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
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        if editor_has_visible_buffer(&editor, "*mixer*") {
                            editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                        }
                        prev_pattern_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        prev_track_button_states = track_button_state_snapshot(&state);
                        prev_track_playheads = track_playheads_snapshot(&state, &app);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Launched track {} pattern {}",
                            track + 1,
                            pattern_id
                        )));
                    }
                    "switch-pattern" => {
                        let profile_switch = pattern_switch_profile_enabled();
                        let profile_total_started = Instant::now();
                        if let Value::Map(ref map) = payload {
                            let idx = map.get("idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            if let Some(idx) = idx {
                                let quantize_label =
                                    extract_string_from_payload(&payload, "quantize")
                                        .unwrap_or_else(|| "off".to_string());
                                let Some(quantize) = sequencer::quantized_launch::LaunchQuantize::from_transport_label(&quantize_label) else {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Unknown scene launch quantization: {quantize_label}"
                                    )));
                                    continue;
                                };
                                if quantize != sequencer::quantized_launch::LaunchQuantize::Off {
                                    match state.schedule_quantized_pattern_launch(
                                        sequencer::quantized_launch::PatternLaunchTarget::Scene {
                                            scene: idx,
                                        },
                                        quantize,
                                        sequencer::quantized_launch::QuantizedLaunchOwner::Transport,
                                    ) {
                                        Ok(token) => editor.handle_host_event(HostEvent::Status(
                                            format!(
                                                "Queued scene {} at {} (launch {})",
                                                idx + 1,
                                                quantize.transport_label(),
                                                token
                                            ),
                                        )),
                                        Err(error) => editor.handle_host_event(HostEvent::Error(
                                            format!("Could not queue scene launch: {error:?}"),
                                        )),
                                    }
                                    continue;
                                }
                                let switch_bus_elapsed = Duration::ZERO;
                                let state_switch_elapsed;
                                let apply_samples_elapsed = Duration::ZERO;
                                let restored_defaults_elapsed = Duration::ZERO;
                                let mut sync_names_pattern_elapsed = Duration::ZERO;
                                let mut sync_current_steps_elapsed = Duration::ZERO;
                                let mut sync_sequencer_elapsed = Duration::ZERO;
                                let mut sync_expanded_elapsed = Duration::ZERO;
                                let mut sync_piano_elapsed = Duration::ZERO;
                                let mut sync_step_params_elapsed = Duration::ZERO;
                                let mut sync_mixer_elapsed = Duration::ZERO;
                                let mut sync_fx_lists_elapsed = Duration::ZERO;
                                let mut sync_effects_elapsed = Duration::ZERO;
                                let mut sync_midi_effects_elapsed = Duration::ZERO;
                                let mut sync_instrument_panel_elapsed = Duration::ZERO;
                                let mut sync_accumulators_elapsed = Duration::ZERO;
                                let mut sync_track_params_elapsed = Duration::ZERO;
                                let mut sync_fx_bindings_elapsed = Duration::ZERO;
                                let mut sync_plocks_sidebar_elapsed = Duration::ZERO;
                                let mut reactive_elapsed = Duration::ZERO;
                                let mut side_effects_elapsed = Duration::ZERO;
                                let started = Instant::now();
                                let switched = app.apply_manual_pattern_launch(
                                    &sequencer::quantized_launch::PatternLaunchTarget::Scene {
                                        scene: idx,
                                    },
                                );
                                state_switch_elapsed = started.elapsed();
                                let pattern_changed = switched.is_ok();
                                if switched.is_ok() {
                                    let ct = current_track.load(Ordering::Relaxed);
                                    let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
                                    let sequencer_visible =
                                        editor_has_visible_buffer(&editor, "*sequencer*");
                                    let rt = editor.runtime_mut();
                                    let started = Instant::now();
                                    sync_shared_track_collapsed(&track_collapsed, &app);
                                    sync_track_name_state(rt, &mut track_names, &app);
                                    sync_pattern_state(rt, &state);
                                    sync_names_pattern_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                                    sync_current_steps_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    sync_all_track_sequencer_state(
                                        rt,
                                        &state,
                                        &app,
                                        ct,
                                        &selected_steps,
                                    );
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
                                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                                    sync_mixer_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    if fx_visible {
                                        let sub_started = Instant::now();
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
                                        sync_effects_elapsed = sub_started.elapsed();

                                        let sub_started = Instant::now();
                                        rt.set_reactive(
                                            "SEQ",
                                            "midi-effects",
                                            build_midi_effects_value(&state, ct, &selected_steps),
                                        );
                                        sync_midi_effects_elapsed = sub_started.elapsed();

                                        let sub_started = Instant::now();
                                        rt.set_reactive(
                                            "SEQ",
                                            "instrument-panel",
                                            build_instrument_panel_value(&app, ct, &selected_steps),
                                        );
                                        sync_instrument_panel_elapsed = sub_started.elapsed();

                                        let sub_started = Instant::now();
                                        *accumulator_names.lock().unwrap() =
                                            build_accumulator_names(&app);
                                        sync_accumulators_elapsed = sub_started.elapsed();
                                    } else {
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    sync_fx_lists_elapsed = started.elapsed();
                                    let started = Instant::now();
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
                                    let started = Instant::now();
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            ct,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_sidebar_browser(rt, &app, ct);
                                    sync_plocks_sidebar_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    rt.run_reactive_cycle();
                                    reactive_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    editor.refresh_runtime_side_effects();
                                    side_effects_elapsed = started.elapsed();
                                    if editor_has_visible_buffer(&editor, "*mixer*") {
                                        editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                                    }
                                    prev_pattern_epoch =
                                        state.transport.pattern_epoch.load(Ordering::Relaxed);
                                    prev_track_button_states = track_button_state_snapshot(&state);
                                    prev_track_playheads = track_playheads_snapshot(&state, &app);
                                }
                                if profile_switch {
                                    eprintln!(
                                        "[pattern-switch-profile][host] idx={} changed={} total={:.2}ms switch_bus={:.2}ms state_switch={:.2}ms apply_samples={:.2}ms defaults={:.2}ms names_pattern={:.2}ms current_steps={:.2}ms sequencer_bindings={:.2}ms expanded_step_viewports={:.2}ms piano={:.2}ms step_params={:.2}ms mixer={:.2}ms fx_lists={:.2}ms effects={:.2}ms midi_effects={:.2}ms instrument_panel={:.2}ms accumulators={:.2}ms track_params={:.2}ms fx_bindings={:.2}ms plocks_sidebar={:.2}ms reactive={:.2}ms side_effects={:.2}ms",
                                        idx,
                                        pattern_changed,
                                        duration_ms(profile_total_started.elapsed()),
                                        duration_ms(switch_bus_elapsed),
                                        duration_ms(state_switch_elapsed),
                                        duration_ms(apply_samples_elapsed),
                                        duration_ms(restored_defaults_elapsed),
                                        duration_ms(sync_names_pattern_elapsed),
                                        duration_ms(sync_current_steps_elapsed),
                                        duration_ms(sync_sequencer_elapsed),
                                        duration_ms(sync_expanded_elapsed),
                                        duration_ms(sync_piano_elapsed),
                                        duration_ms(sync_step_params_elapsed),
                                        duration_ms(sync_mixer_elapsed),
                                        duration_ms(sync_fx_lists_elapsed),
                                        duration_ms(sync_effects_elapsed),
                                        duration_ms(sync_midi_effects_elapsed),
                                        duration_ms(sync_instrument_panel_elapsed),
                                        duration_ms(sync_accumulators_elapsed),
                                        duration_ms(sync_track_params_elapsed),
                                        duration_ms(sync_fx_bindings_elapsed),
                                        duration_ms(sync_plocks_sidebar_elapsed),
                                        duration_ms(reactive_elapsed),
                                        duration_ms(side_effects_elapsed),
                                    );
                                }
                            }
                        }
                    }
                    "rename-scene" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Could not rename scene: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) if *n >= 0.0 => Some(*n as usize),
                            _ => None,
                        });
                        let name = map.get("name").and_then(|cell| match &*cell.borrow() {
                            Value::String(name) => Some(name.clone()),
                            _ => None,
                        });
                        let renamed = match (scene, name) {
                            (Some(scene), Some(name)) => app.apply_recorded_scene_structure_mutation(
                                "Rename scene",
                                |app| app.state.rename_scene(scene, name)
                                    .then_some(())
                                    .ok_or_else(|| "Scene name or index is invalid".to_string()),
                            ),
                            _ => Err("Scene or name is missing".to_string()),
                        };
                        match renamed {
                            Ok(()) => {
                                let rt = editor.runtime_mut();
                                sync_pattern_state(rt, &state);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                "Could not rename scene: {error}"
                            ))),
                        }
                    }
                    "reorder-scene" => {
                        let source = extract_usize_from_payload(&payload, "source");
                        let target = extract_usize_from_payload(&payload, "target");
                        match (source, target) {
                            (Some(source), Some(target)) => {
                                let reordered = app.apply_recorded_scene_structure_mutation(
                                    "Reorder scene",
                                    |app| app.state.reorder_scene(source, target)
                                        .ok_or_else(|| "Scene index is out of range".to_string()),
                                );
                                if reordered.is_ok() {
                                    let rt = editor.runtime_mut();
                                    sync_pattern_state(rt, &state);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved scene {} to {}",
                                        source + 1,
                                        target + 1
                                    )));
                                } else {
                                    editor.handle_host_event(HostEvent::Status(
                                        "Could not reorder scenes: scene index out of range"
                                            .to_string(),
                                    ));
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "Could not reorder scenes: invalid drag payload".to_string(),
                            )),
                        }
                    }
                    "propagate-current-track-to-all-patterns" => {
                        let track = match payload {
                            Value::Number(n) => n as usize,
                            _ => current_track.load(Ordering::Relaxed),
                        };
                        let num_patterns = state.scene_count();
                        if track >= app.tracks.len() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track {} is out of range",
                                track + 1
                            )));
                        } else if num_patterns <= 1 {
                            editor.handle_host_event(HostEvent::Status(
                                "Nothing to propagate: only one pattern exists".to_string(),
                            ));
                        } else if app.apply_recorded_scene_structure_mutation(
                            "Propagate track pattern",
                            |app| app.state.propagate_track_to_all_patterns(
                                track,
                                app.tracks.len(),
                                &app.graph.track_buffer_ids,
                                &app.graph.track_sample_rates,
                                &app.tracks,
                                &app.graph.track_instrument_types,
                            ).then_some(()).ok_or_else(|| format!(
                                "Could not propagate track {}", track + 1
                            )),
                        ).is_ok() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Propagated track {} to {} patterns",
                                track + 1,
                                num_patterns
                            )));
                        } else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Failed to propagate track {}",
                                track + 1
                            )));
                        }
                    }
                    "clone-pattern" => {
                        let num_tracks = app.tracks.len();
                        let created = app.apply_recorded_scene_structure_mutation(
                            "Create scene",
                            |app| {
                                let source_pattern = app.state.current_scene_index();
                                let new_idx = app.state.clone_pattern(
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.graph.track_sample_rates,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                );
                                app.graph_controller().sync_current_pattern_mod_routes();
                                app.clone_bus_pattern_from_to(source_pattern, new_idx);
                                Ok(new_idx)
                            },
                        );
                        let Ok(new_idx) = created else {
                            editor.handle_host_event(HostEvent::Status(
                                "Could not create scene".to_string(),
                            ));
                            continue;
                        };
                        let rt = editor.runtime_mut();
                        sync_pattern_state(rt, &state);
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cloned pattern {}",
                            new_idx + 1
                        )));
                    }
                    "delete-pattern" => {
                        let num_tracks = app.tracks.len();
                        let deleted_pattern = app.state.current_scene_index();
                        let deleted = app.apply_recorded_scene_structure_mutation(
                            "Delete scene",
                            |app| {
                                let sample_ids = app.state.delete_pattern(
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.graph.track_sample_rates,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                ).ok_or_else(|| "The last scene cannot be deleted".to_string())?;
                                app.handle_scene_deleted(deleted_pattern);
                                app.graph_controller().apply_sample_ids(&sample_ids);
                                app.graph_controller().sync_current_pattern_mod_routes();
                                app.push_all_restored_defaults();
                                let new_pattern = app.state.current_scene_index();
                                app.delete_bus_pattern_at(deleted_pattern, new_pattern);
                                Ok(())
                            },
                        );
                        if deleted.is_ok() {
                            let ct = current_track.load(Ordering::Relaxed);
                            let rt = editor.runtime_mut();
                            sync_shared_track_collapsed(&track_collapsed, &app);
                            sync_track_name_state(rt, &mut track_names, &app);
                            sync_pattern_state(rt, &state);
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                            sync_step_param_lists(rt, &state, ct);
                            sync_track_mixer_state(rt, &app, &state);
                            sync_bus_mixer_state(rt, &app);
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    "agent-accept" => {
                        let conv_id = match payload {
                            Value::Number(id) if id >= 1.0 => id as sequencer::agent::store::ConvId,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-accept: expected conversation id".to_string(),
                                ));
                                continue;
                            }
                        };
                        let snapshot = app.agent_store.snapshot(conv_id);
                        let apply_as_effect =
                            match snapshot.as_ref().map(|snapshot| &snapshot.state) {
                                Some(state) => match state.kind {
                                    sequencer::agent::store::AgentKind::Effect => true,
                                    sequencer::agent::store::AgentKind::Instrument => false,
                                    sequencer::agent::store::AgentKind::General => {
                                        state.effect_draft.is_some()
                                            || state.accepted_effect_target.is_some()
                                    }
                                },
                                None => false,
                            };
                        if !apply_as_effect {
                            match apply_agent_draft_to_owned_instrument(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                            ) {
                                Ok(result) => {
                                    let verb = if result.created_track {
                                        "Accepted agent draft as track"
                                    } else {
                                        "Updated agent draft on track"
                                    };
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "{verb} {}",
                                        result.track_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        } else {
                            match apply_agent_draft_to_effect_slot(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                            ) {
                                Ok(result) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Accepted agent effect as track {} slot {}",
                                        result.track_index + 1,
                                        result.slot_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        }
                    }
                    "agent-send" => {
                        let Value::Map(map) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "agent-send: expected payload map".to_string(),
                            ));
                            continue;
                        };
                        let conv_id = match map.get("id").map(|cell| cell.borrow().clone()) {
                            Some(Value::Number(id)) if id >= 1.0 => {
                                id as sequencer::agent::store::ConvId
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-send: expected conversation id".to_string(),
                                ));
                                continue;
                            }
                        };
                        let prompt = match map.get("prompt").map(|cell| cell.borrow().clone()) {
                            Some(Value::String(prompt)) => prompt,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-send: expected prompt string".to_string(),
                                ));
                                continue;
                            }
                        };

                        let needs_stub = app
                            .agent_store
                            .snapshot(conv_id)
                            .map(|snapshot| {
                                let state = snapshot.state;
                                state.kind == sequencer::agent::store::AgentKind::Instrument
                                    && state.draft.is_none()
                                    && state.stub_instrument_target.is_none()
                                    && state.accepted_instrument_target.is_none()
                                    && state.finalized_instrument_name.is_none()
                            })
                            .unwrap_or(false);
                        if needs_stub {
                            match ensure_agent_instrument_stub_track(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                            ) {
                                Ok(track_index) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Created working instrument track {}",
                                        track_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                    continue;
                                }
                            }
                        }

                        let session_context = metal_agent_session_context(
                            &app,
                            &current_track,
                            &active_delete_target,
                        );
                        if let Err(error) =
                            app.agent_store
                                .send_with_context(conv_id, prompt, session_context)
                        {
                            editor.handle_host_event(HostEvent::Error(error));
                        }
                    }
                    "agent-ensure-instrument-stub" => {
                        let conv_id = match payload {
                            Value::Number(id) if id >= 1.0 => id as sequencer::agent::store::ConvId,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-ensure-instrument-stub: expected conversation id"
                                        .to_string(),
                                ));
                                continue;
                            }
                        };
                        match ensure_agent_instrument_stub_track(
                            &mut app,
                            &mut editor,
                            &state,
                            &current_track,
                            &mut track_names,
                            &track_pan_ids,
                            &record_armed,
                            &selected_steps,
                            &accumulator_names,
                            &cached_track_peak_levels,
                            &cached_bus_peak_levels,
                            &ui_epoch,
                            lg_raw,
                            conv_id,
                        ) {
                            Ok(track_index) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Created working instrument track {}",
                                    track_index + 1
                                )));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                            }
                        }
                    }
                    "agent-finalize" => {
                        let Value::Map(map) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "agent-finalize: expected payload map".to_string(),
                            ));
                            continue;
                        };
                        let conv_id = match map.get("id").map(|cell| cell.borrow().clone()) {
                            Some(Value::Number(id)) if id >= 1.0 => {
                                id as sequencer::agent::store::ConvId
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-finalize: expected conversation id".to_string(),
                                ));
                                continue;
                            }
                        };
                        let requested_name = match map.get("name").map(|cell| cell.borrow().clone())
                        {
                            Some(Value::String(name)) if !name.trim().is_empty() => name,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-finalize: expected non-empty artifact name".to_string(),
                                ));
                                continue;
                            }
                        };
                        let snapshot = app.agent_store.snapshot(conv_id);
                        let finalize_as_effect =
                            match snapshot.as_ref().map(|snapshot| &snapshot.state) {
                                Some(state) => match state.kind {
                                    sequencer::agent::store::AgentKind::Effect => true,
                                    sequencer::agent::store::AgentKind::Instrument => false,
                                    sequencer::agent::store::AgentKind::General => {
                                        state.effect_draft.is_some()
                                            || state.accepted_effect_target.is_some()
                                    }
                                },
                                None => false,
                            };
                        if !finalize_as_effect {
                            match finalize_agent_instrument(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                                &requested_name,
                            ) {
                                Ok(result) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Saved agent artifact {} as track {}",
                                        display_instrument_name(&result.instrument_name),
                                        result.track_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        } else {
                            match finalize_agent_effect(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                                &requested_name,
                            ) {
                                Ok(result) => {
                                    let target = match (result.track_index, result.slot_index) {
                                        (Some(track), Some(slot)) => {
                                            format!(" on track {} slot {}", track + 1, slot + 1)
                                        }
                                        _ => String::new(),
                                    };
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Saved agent effect artifact {}{}",
                                        display_instrument_name(&result.effect_name),
                                        target
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        }
                    }
                    // ── Inline instrument/effect editor commands ──
                    "enter-new-instrument-editor" => {
                        if editor_mode.is_some() || instrument_edit_session.is_some() {
                            editor.handle_host_event(HostEvent::Error(
                                "Close the current editor before creating a new instrument"
                                    .to_string(),
                            ));
                            continue;
                        }
                        let original_track = current_track.load(Ordering::Relaxed);
                        let temp_dir = match create_new_instrument_draft_dir() {
                            Ok(dir) => dir,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                                continue;
                            }
                        };
                        let file_path = temp_dir.join("dsp.lisp");
                        if let Err(error) = std::fs::write(&file_path, NEW_INSTRUMENT_STARTER_DSP) {
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write starter instrument: {error}"
                            )));
                            continue;
                        }

                        let draft_track = match app.add_transient_instrument_track_sync(
                            NEW_INSTRUMENT_DRAFT_NAME,
                            NEW_INSTRUMENT_STARTER_DSP,
                            Some(&temp_dir),
                        ) {
                            Ok(track) => track,
                            Err(error) => {
                                let _ = std::fs::remove_dir_all(&temp_dir);
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to create draft instrument track: {error}"
                                )));
                                continue;
                            }
                        };
                        let _ = app.force_instrument_enabled(draft_track);
                        sync_after_instrument_track_apply(
                            &mut app,
                            &mut editor,
                            &state,
                            draft_track,
                            &current_track,
                            &mut track_names,
                            &track_pan_ids,
                            &record_armed,
                            &selected_steps,
                            &accumulator_names,
                            &cached_track_peak_levels,
                            &cached_bus_peak_levels,
                            &ui_epoch,
                            lg_raw,
                        );

                        let Some(engine_id) = app
                            .graph
                            .track_engine_ids
                            .get(draft_track)
                            .and_then(|id| *id)
                        else {
                            let _ = app.graph_controller().delete_track(draft_track);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(
                                "Draft instrument track has no engine binding".to_string(),
                            ));
                            continue;
                        };

                        let buf_name = "*instrument-patcher:new-instrument*".to_string();
                        editor.remove_buffer_by_name(&buf_name);
                        editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                        let patcher_source =
                            instrument_patcher_buffer_source(&buf_name, &file_path);
                        if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                            let _ = app.graph_controller().delete_track(draft_track);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to build patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        reset_instrument_patcher_state(&file_path);
                        let layout_source = show_instrument_patcher_layout_source(&buf_name);
                        if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                            let _ = app.graph_controller().delete_track(draft_track);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        editor_buffer_name = Some(buf_name.clone());
                        editor_mode = Some("new-instrument".to_string());
                        instrument_edit_session = Some(InstrumentEditSession::begin_create_draft(
                            NEW_INSTRUMENT_DRAFT_NAME.to_string(),
                            file_path,
                            buf_name.clone(),
                            engine_id,
                            NEW_INSTRUMENT_STARTER_DSP.to_string(),
                            temp_dir,
                            draft_track,
                            original_track,
                        ));
                        let rt = editor.runtime_mut();
                        let _ = rt.eval_str("(set! sbrowser-editor-name \"\")");
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String("new-instrument".to_string()),
                        );
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(buf_name.clone()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-instrument-run-mode",
                            Value::String("instrument".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Created draft instrument track {}",
                            draft_track + 1
                        )));
                    }

                    "set-draft-instrument-run-mode" => {
                        let Some(session) = instrument_edit_session.as_mut() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No instrument edit session is active".to_string(),
                            ));
                            continue;
                        };
                        if !matches!(&session.mode, InstrumentEditMode::CreateDraft { .. }) {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Run mode can only be changed for draft instruments"
                                        .to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let requested = extract_string_from_payload(&payload, "run-mode")
                            .unwrap_or_else(|| "instrument".to_string());
                        let Some(run_mode) = instrument_run_mode_from_label(&requested) else {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Unknown instrument run mode '{requested}'")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        };
                        match app
                            .graph_controller()
                            .set_track_instrument_run_mode(session.track, run_mode)
                        {
                            Ok(()) => {
                                session.run_mode = run_mode;
                                if let Some(engine_id) = app.graph.track_engine_ids[session.track] {
                                    session.engine_id = engine_id;
                                }
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-instrument-run-mode",
                                    Value::String(instrument_run_mode_label(run_mode).to_string()),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(String::new()),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Draft instrument mode: {}",
                                    match run_mode {
                                        CustomInstrumentRunMode::Instrument => "Instrument",
                                        CustomInstrumentRunMode::FreePatch => "Free Patch",
                                    }
                                )));
                            }
                            Err(error) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                            }
                        }
                    }

                    "save-active-editor-macro" => {
                        let result = if let Some(session) = instrument_edit_session.as_mut() {
                            apply_active_instrument_editor_macro_action(session)
                        } else if let Some(session) = effect_edit_session.as_mut() {
                            apply_active_effect_editor_macro_action(session)
                        } else {
                            Err("No patch editor session is active".to_string())
                        };
                        match result {
                            Ok(Some(result)) => {
                                let action_status = macro_library_action_status(&result);
                                let editor_macro_action = instrument_edit_session
                                    .as_ref()
                                    .and_then(active_instrument_editor_macro_action)
                                    .or_else(|| {
                                        effect_edit_session
                                            .as_ref()
                                            .and_then(active_effect_editor_macro_action)
                                    });
                                let editor_macro_action =
                                    editor_macro_action_strings(editor_macro_action.as_ref());
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
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(String::new()),
                                );
                                prev_editor_macro_action = editor_macro_action;
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.refresh_visible_layouts_for_buffer_named("*samples*");
                                editor.handle_host_event(HostEvent::Status(action_status));
                            }
                            Ok(None) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String("No active macro is selected".to_string()),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.refresh_visible_layouts_for_buffer_named("*samples*");
                            }
                            Err(error) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.refresh_visible_layouts_for_buffer_named("*samples*");
                            }
                        }
                    }

                    "save-new-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.trim().to_string();
                                    if inst_name.is_empty() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String("Name cannot be empty".to_string()),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let Some(session) = instrument_edit_session.as_ref() else {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "No draft instrument session is active".to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    };
                                    if !matches!(
                                        &session.mode,
                                        InstrumentEditMode::CreateDraft { .. }
                                    ) {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Current editor session is not a draft instrument"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if !session.visible_revision_valid {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Cannot finalize: the current patch has errors"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let flushed_macros =
                                        match flush_staged_instrument_library_macro_edits(session) {
                                            Ok(macros) => macros,
                                            Err(error) => {
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "editor-error",
                                                    Value::String(format!(
                                                        "Failed to save library macro edits: {error}"
                                                    )),
                                                );
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                continue;
                                            }
                                        };

                                    let final_slug =
                                        sequencer::agent::actions::normalize_patch_name(
                                            &inst_name,
                                            "new-instrument",
                                        );
                                    let final_name = format!("{final_slug}/");
                                    let (final_dir, legacy_file) =
                                        finalized_instrument_storage_paths(&final_slug);
                                    if final_dir.exists() || legacy_file.exists() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Instrument '{final_slug}' already exists"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let source = session.last_valid_source.clone();
                                    let draft_track = match &session.mode {
                                        InstrumentEditMode::CreateDraft { draft_track, .. } => {
                                            *draft_track
                                        }
                                        InstrumentEditMode::EditExisting { .. } => unreachable!(),
                                    };
                                    if let Err(error) =
                                        sequencer::lisp_host::save_instrument(&final_name, &source)
                                    {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to save finalized instrument: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if let Err(error) =
                                        sequencer::lisp_host::save_instrument_run_mode(
                                            &final_name,
                                            session.run_mode,
                                        )
                                    {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to save finalized instrument mode: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let target_dsp = final_dir.join("dsp.lisp");
                                    if let Some(layout) = session.last_valid_layout.as_deref() {
                                        if let Err(error) =
                                            write_patcher_layout_sidecar(&target_dsp, layout)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save finalized instrument layout: {error}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    } else if let InstrumentEditMode::CreateDraft {
                                        temp_dir, ..
                                    } = &session.mode
                                    {
                                        let source_dsp = temp_dir.join("dsp.lisp");
                                        if let Err(error) =
                                            copy_patcher_layout_sidecar(&source_dsp, &target_dsp)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save finalized instrument layout: {error}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    }
                                    if let Err(error) = app.replace_custom_instrument_track_sync(
                                        draft_track,
                                        &final_name,
                                        &source,
                                    ) {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to load finalized instrument: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if let Err(error) =
                                        app.graph_controller().set_track_instrument_run_mode(
                                            draft_track,
                                            session.run_mode,
                                        )
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to apply finalized instrument mode: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let session =
                                        instrument_edit_session.take().expect("session checked");
                                    if let InstrumentEditMode::CreateDraft { temp_dir, .. } =
                                        session.mode
                                    {
                                        let _ = std::fs::remove_dir_all(temp_dir);
                                    }
                                    reset_instrument_patcher_state(&session.path);
                                    let buf_name = session.buffer_name;
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
                                    if let Some(status) =
                                        staged_library_macro_flush_status(&flushed_macros)
                                    {
                                        editor.handle_host_event(HostEvent::Status(status));
                                    }
                                    editor_buffer_name = None;
                                    editor_mode = None;
                                    current_track.store(draft_track, Ordering::Relaxed);
                                    app.ui.cursor_track = draft_track;
                                    track_names = app.tracks.clone();
                                    sync_shared_track_collapsed(&track_collapsed, &app);

                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-instrument-run-mode",
                                        Value::String("instrument".to_string()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&app.tracks),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(
                                            &app,
                                            draft_track,
                                            &selected_steps,
                                        ),
                                    );
                                    sync_sidebar_browser(rt, &app, draft_track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Finalized instrument '{}' on track {}",
                                        display_instrument_name(&final_name),
                                        draft_track + 1
                                    )));
                                }
                            }
                        }
                    }

                    "enter-edit-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    let file_path =
                                        match sequencer::lisp_host::instrument_source_path(
                                            &inst_name,
                                        ) {
                                            Ok(path) => path,
                                            Err(e) => {
                                                editor.handle_host_event(HostEvent::Error(
                                                    format!("Instrument file not found: {e}"),
                                                ));
                                                continue;
                                            }
                                        };
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Instrument file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let track = current_track.load(Ordering::Relaxed);
                                    let Some(engine_id) =
                                        app.graph.track_engine_ids.get(track).and_then(|id| *id)
                                    else {
                                        editor.handle_host_event(HostEvent::Error(
                                            "Current instrument track has no engine binding"
                                                .to_string(),
                                        ));
                                        continue;
                                    };
                                    let persisted_source = match std::fs::read_to_string(&file_path)
                                    {
                                        Ok(source) => source,
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to read '{}': {error}",
                                                file_path.display()
                                            )));
                                            continue;
                                        }
                                    };
                                    let run_mode =
                                        match sequencer::lisp_host::load_instrument_run_mode(
                                            &inst_name,
                                        ) {
                                            Ok(run_mode) => run_mode,
                                            Err(error) => {
                                                editor.handle_host_event(HostEvent::Error(
                                                    format!(
                                                        "Failed to load instrument mode: {error}"
                                                    ),
                                                ));
                                                continue;
                                            }
                                        };
                                    sync_after_instrument_track_apply(
                                        &mut app,
                                        &mut editor,
                                        &state,
                                        track,
                                        &current_track,
                                        &mut track_names,
                                        &track_pan_ids,
                                        &record_armed,
                                        &selected_steps,
                                        &accumulator_names,
                                        &cached_track_peak_levels,
                                        &cached_bus_peak_levels,
                                        &ui_epoch,
                                        lg_raw,
                                    );
                                    let buf_name = format!("*instrument-patcher:{inst_name}*");
                                    editor.remove_buffer_by_name(&buf_name);
                                    editor.create_scratch_buffer(
                                        &buf_name,
                                        "",
                                        BufferMode::ESeqLisp,
                                    );
                                    let patcher_source =
                                        instrument_patcher_buffer_source(&buf_name, &file_path);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&patcher_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to build patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    reset_instrument_patcher_state(&file_path);
                                    let layout_source =
                                        show_instrument_patcher_layout_source(&buf_name);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&layout_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to show patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    editor.refresh_runtime_side_effects();
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-instrument".to_string());
                                    instrument_edit_session =
                                        Some(InstrumentEditSession::begin_edit_existing(
                                            inst_name,
                                            file_path,
                                            buf_name.clone(),
                                            engine_id,
                                            track,
                                            persisted_source,
                                            run_mode,
                                        ));
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String("edit-instrument".to_string()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(buf_name),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-instrument-run-mode",
                                        Value::String(
                                            instrument_run_mode_label(run_mode).to_string(),
                                        ),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    refresh_visible_track_topology_layouts(&mut editor);
                                }
                            }
                        }
                    }

                    "update-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    if let Some(session) = instrument_edit_session.as_ref() {
                                        if !session.visible_revision_valid {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(
                                                    "Cannot save: the current patch has errors"
                                                        .to_string(),
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                        let flushed_macros =
                                            match flush_staged_instrument_library_macro_edits(
                                                session,
                                            ) {
                                                Ok(macros) => macros,
                                                Err(error) => {
                                                    let rt = editor.runtime_mut();
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "editor-error",
                                                        Value::String(format!(
                                                            "Failed to save library macro edits: {error}"
                                                        )),
                                                    );
                                                    rt.run_reactive_cycle();
                                                    editor.refresh_runtime_side_effects();
                                                    continue;
                                                }
                                            };
                                        if let Err(e) = std::fs::write(
                                            &session.path,
                                            &session.last_valid_source,
                                        ) {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!("Failed to save: {e}")),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                        if let Some(layout) = session.last_valid_layout.as_deref() {
                                            if let Err(e) =
                                                write_patcher_layout_sidecar(&session.path, layout)
                                            {
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "editor-error",
                                                    Value::String(format!(
                                                        "Failed to save layout: {e}"
                                                    )),
                                                );
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                continue;
                                            }
                                        }

                                        let buf_name = session.buffer_name.clone();
                                        reset_instrument_patcher_state(&session.path);
                                        if let Err(error) = editor
                                            .runtime_mut()
                                            .eval_str(restore_instrument_patcher_layout_source())
                                        {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to restore main editor layout: {error:?}"
                                            )));
                                            continue;
                                        }
                                        editor.refresh_runtime_side_effects();
                                        editor.remove_buffer_by_name(&buf_name);
                                        if let Some(status) =
                                            staged_library_macro_flush_status(&flushed_macros)
                                        {
                                            editor.handle_host_event(HostEvent::Status(status));
                                        }
                                        editor_buffer_name = None;
                                        editor_mode = None;
                                        instrument_edit_session = None;

                                        let ct = current_track.load(Ordering::Relaxed);
                                        track_names[ct] = inst_name.clone();
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-mode",
                                            Value::String(String::new()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(String::new()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-buffer-name",
                                            Value::String(String::new()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-instrument-run-mode",
                                            Value::String("instrument".to_string()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "track-names",
                                            build_track_names(&track_names),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Saved instrument '{inst_name}'"
                                        )));
                                        continue;
                                    }
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    if let Err(e) =
                                        sequencer::lisp_host::save_instrument(&inst_name, &source)
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("Failed to save: {e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    // Try hot-swap FIRST — stay in editor on failure
                                    app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                    match app
                                        .replace_current_custom_instrument_sync(&inst_name, &source)
                                    {
                                        Ok(()) => {
                                            // Success — close editor
                                            editor
                                                .swap_buffer_in_tile_showing(&buf_name, "*metal*");
                                            editor.remove_buffer_by_name(&buf_name);
                                            editor_buffer_name = None;
                                            editor_mode = None;

                                            let ct = current_track.load(Ordering::Relaxed);
                                            track_names[ct] = inst_name.clone();
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-active",
                                                Value::Bool(false),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-mode",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-buffer-name",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-instrument-run-mode",
                                                Value::String("instrument".to_string()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-names",
                                                build_track_names(&track_names),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "instrument-panel",
                                                build_instrument_panel_value(
                                                    &app,
                                                    ct,
                                                    &selected_steps,
                                                ),
                                            );
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
                                                build_midi_effects_value(
                                                    &state,
                                                    ct,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Hot-swapped instrument '{inst_name}'"
                                            )));
                                        }
                                        Err(e) => {
                                            // Compile failed — stay in editor, show error
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!("{e}")),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    "preview-instrument-patch" => {
                        let Some(session) = instrument_edit_session.as_mut() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No instrument edit session is active".to_string(),
                            ));
                            continue;
                        };
                        let status = extract_string_from_payload(&payload, "status")
                            .unwrap_or_else(|| "invalid".to_string());
                        if status == "agentic-submit" {
                            let Some(path) = extract_string_from_payload(&payload, "path") else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing patch path".to_string(),
                                ));
                                continue;
                            };
                            let Some(bubble_id) =
                                extract_string_from_payload(&payload, "bubble-id")
                            else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing bubble id".to_string(),
                                ));
                                continue;
                            };
                            let generation = extract_usize_from_payload(&payload, "generation")
                                .unwrap_or(0) as u64;
                            let prompt =
                                extract_string_from_payload(&payload, "prompt").unwrap_or_default();
                            let macro_name = extract_string_from_payload(&payload, "macro-name")
                                .unwrap_or_else(|| "agentic-macro".to_string());
                            let target = extract_string_from_payload(&payload, "target")
                                .unwrap_or_else(|| "create-macro".to_string());
                            let intent = match extract_string_from_payload(&payload, "intent")
                                .as_deref()
                            {
                                Some("effect") => {
                                    eseqlisp::widget_render::patcher::PatcherIntent::Effect
                                }
                                _ => eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                            };
                            let task_key = format!("{path}::{bubble_id}");
                            eprintln!(
                                "[agentic-bubble] host submit key={} generation={} intent={:?} macro={} prompt={:?}",
                                task_key, generation, intent, macro_name, prompt
                            );
                            let (tx, rx) = std::sync::mpsc::channel();
                            let follow_up = if target == "edit-macro" {
                                let existing_macro_name =
                                    extract_string_from_payload(&payload, "existing-macro-name")
                                        .unwrap_or_else(|| macro_name.clone());
                                let params =
                                    extract_string_from_payload(&payload, "existing-macro-params")
                                        .unwrap_or_default()
                                        .split_whitespace()
                                        .map(str::to_string)
                                        .collect::<Vec<_>>();
                                let source =
                                    extract_string_from_payload(&payload, "existing-macro-source")
                                        .unwrap_or_default();
                                Some(sequencer::agent::agentic_bubble::AgenticBubbleFollowUp {
                                    macro_name: existing_macro_name,
                                    params,
                                    source,
                                })
                            } else {
                                None
                            };
                            let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                                prompt,
                                suggested_macro_name: macro_name.clone(),
                                follow_up,
                            };
                            std::thread::spawn(move || {
                                let result =
                                    sequencer::agent::agentic_bubble::generate_agentic_bubble_macro(
                                        request,
                                    );
                                let _ = tx.send(result);
                            });
                            pending_agentic_bubbles.insert(
                                task_key,
                                PendingAgenticBubble {
                                    path: PathBuf::from(path),
                                    intent,
                                    bubble_id,
                                    generation,
                                    receiver: rx,
                                },
                            );
                            editor.handle_host_event(HostEvent::Status(
                                "Agentic bubble working...".to_string(),
                            ));
                            continue;
                        }
                        if status == "layout" {
                            if let Some(layout) = extract_string_from_payload(&payload, "layout") {
                                session.last_valid_layout = Some(layout);
                                if let Some(pending) = pending_instrument_preview.as_mut() {
                                    pending.layout = session.last_valid_layout.clone();
                                }
                            }
                            continue;
                        }
                        if status != "valid" {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_instrument_preview = None;
                            let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                                .unwrap_or_else(|| "Patch writeback failed".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-error", Value::String(diagnostic));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let Some(source) = extract_string_from_payload(&payload, "source") else {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_instrument_preview = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Patch preview did not include emitted source".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        };

                        let compile_source =
                            extract_string_from_payload(&payload, "compile-source")
                                .unwrap_or_else(|| source.clone());
                        let layout = extract_string_from_payload(&payload, "layout");
                        session.preview_generation = session.preview_generation.wrapping_add(1);
                        session.visible_revision_valid = false;
                        let generation = session.preview_generation;
                        let sample_rate = app.graph.sample_rate;
                        let asset_base = session.path.parent().map(|parent| parent.to_path_buf());
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result =
                                sequencer::lisp_host::compile_and_load_instrument_with_origin(
                                    &compile_source,
                                    sample_rate,
                                    asset_base.as_deref(),
                                    sequencer::lisp_host::DGenSourceOrigin::Draft,
                                );
                            let _ = tx.send(result);
                        });
                        pending_instrument_preview = Some(PendingInstrumentPreview {
                            generation,
                            source,
                            layout,
                            receiver: rx,
                        });
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Preview compiling...".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }

                    "preview-effect-patch" => {
                        let Some(session) = effect_edit_session.as_mut() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No effect edit session is active".to_string(),
                            ));
                            continue;
                        };
                        let status = extract_string_from_payload(&payload, "status")
                            .unwrap_or_else(|| "invalid".to_string());
                        if status == "agentic-submit" {
                            let Some(path) = extract_string_from_payload(&payload, "path") else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing patch path".to_string(),
                                ));
                                continue;
                            };
                            let Some(bubble_id) =
                                extract_string_from_payload(&payload, "bubble-id")
                            else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing bubble id".to_string(),
                                ));
                                continue;
                            };
                            let generation = extract_usize_from_payload(&payload, "generation")
                                .unwrap_or(0) as u64;
                            let prompt =
                                extract_string_from_payload(&payload, "prompt").unwrap_or_default();
                            let macro_name = extract_string_from_payload(&payload, "macro-name")
                                .unwrap_or_else(|| "agentic-macro".to_string());
                            let target = extract_string_from_payload(&payload, "target")
                                .unwrap_or_else(|| "create-macro".to_string());
                            let task_key = format!("{path}::{bubble_id}");
                            let (tx, rx) = std::sync::mpsc::channel();
                            let follow_up = if target == "edit-macro" {
                                let existing_macro_name =
                                    extract_string_from_payload(&payload, "existing-macro-name")
                                        .unwrap_or_else(|| macro_name.clone());
                                let params =
                                    extract_string_from_payload(&payload, "existing-macro-params")
                                        .unwrap_or_default()
                                        .split_whitespace()
                                        .map(str::to_string)
                                        .collect::<Vec<_>>();
                                let source =
                                    extract_string_from_payload(&payload, "existing-macro-source")
                                        .unwrap_or_default();
                                Some(sequencer::agent::agentic_bubble::AgenticBubbleFollowUp {
                                    macro_name: existing_macro_name,
                                    params,
                                    source,
                                })
                            } else {
                                None
                            };
                            let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                                prompt,
                                suggested_macro_name: macro_name,
                                follow_up,
                            };
                            std::thread::spawn(move || {
                                let result =
                                    sequencer::agent::agentic_bubble::generate_agentic_bubble_macro(
                                        request,
                                    );
                                let _ = tx.send(result);
                            });
                            pending_agentic_bubbles.insert(
                                task_key,
                                PendingAgenticBubble {
                                    path: PathBuf::from(path),
                                    intent: eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                                    bubble_id,
                                    generation,
                                    receiver: rx,
                                },
                            );
                            editor.handle_host_event(HostEvent::Status(
                                "Agentic bubble working...".to_string(),
                            ));
                            continue;
                        }
                        if status == "layout" {
                            if let Some(layout) = extract_string_from_payload(&payload, "layout") {
                                session.last_valid_layout = Some(layout);
                                if let Some(pending) = pending_effect_preview.as_mut() {
                                    pending.layout = session.last_valid_layout.clone();
                                }
                            }
                            continue;
                        }
                        if status != "valid" {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_effect_preview = None;
                            let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                                .unwrap_or_else(|| "Patch writeback failed".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-error", Value::String(diagnostic));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let Some(source) = extract_string_from_payload(&payload, "source") else {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_effect_preview = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Patch preview did not include emitted source".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        };

                        let compile_source =
                            extract_string_from_payload(&payload, "compile-source")
                                .unwrap_or_else(|| source.clone());
                        let layout = extract_string_from_payload(&payload, "layout");
                        session.preview_generation = session.preview_generation.wrapping_add(1);
                        session.visible_revision_valid = false;
                        let generation = session.preview_generation;
                        let sample_rate = app.graph.sample_rate;
                        let asset_base = session.path.parent().map(|parent| parent.to_path_buf());
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result = sequencer::lisp_host::compile_and_load_with_origin(
                                &compile_source,
                                sample_rate,
                                asset_base.as_deref(),
                                sequencer::lisp_host::DGenSourceOrigin::Draft,
                            );
                            let _ = tx.send(result);
                        });
                        pending_effect_preview = Some(PendingEffectPreview {
                            generation,
                            source,
                            layout,
                            receiver: rx,
                        });
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Preview compiling...".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }

                    "toggle-instrument-patcher-source" => {
                        let (buffer_name, path, last_valid_source) =
                            if let Some(session) = instrument_edit_session.as_ref() {
                                (
                                    session.buffer_name.clone(),
                                    session.path.clone(),
                                    session.last_valid_source.clone(),
                                )
                            } else if let Some(session) = effect_edit_session.as_ref() {
                                (
                                    session.buffer_name.clone(),
                                    session.path.clone(),
                                    session.last_valid_source.clone(),
                                )
                            } else {
                                editor.handle_host_event(HostEvent::Status(
                                    "No patch edit session is active".to_string(),
                                ));
                                continue;
                            };
                        if !path.exists() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Patch source no longer exists: {}",
                                path.display()
                            )));
                            continue;
                        }
                        let source_buffer_name =
                            eseqlisp::widget_render::patcher::emitted_source_buffer_name(
                                &path.to_string_lossy(),
                            );
                        let layout_source =
                            if editor_has_visible_buffer(&editor, &source_buffer_name) {
                                show_instrument_patcher_layout_source(&buffer_name)
                            } else {
                                let source_buffer_name = match editor
                                    .upsert_patcher_emitted_source_buffer(
                                        &buffer_name,
                                        &path,
                                        &last_valid_source,
                                    ) {
                                    Ok(name) => name,
                                    Err(error) => {
                                        editor.handle_host_event(HostEvent::Error(error));
                                        continue;
                                    }
                                };
                                show_instrument_patcher_source_layout_source(
                                    &buffer_name,
                                    &source_buffer_name,
                                )
                            };
                        match editor.runtime_mut().eval_str(&layout_source) {
                            Ok(_) => editor.refresh_runtime_side_effects(),
                            Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch source layout: {error:?}"
                            ))),
                        }
                    }

                    "enter-new-effect-editor" => {
                        if editor_mode.is_some()
                            || instrument_edit_session.is_some()
                            || effect_edit_session.is_some()
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Close the current editor before creating a new effect"
                            )));
                            continue;
                        }
                        if app.tracks.is_empty() {
                            editor.handle_host_event(HostEvent::Error(
                                "No current track is available for a new effect".to_string(),
                            ));
                            continue;
                        }
                        let track = current_track.load(Ordering::Relaxed);
                        app.ui.cursor_track = track;
                        let Some(slot) = app.next_free_custom_slot() else {
                            editor.handle_host_event(HostEvent::Error(
                                "No free effect slots available".to_string(),
                            ));
                            continue;
                        };
                        let temp_dir = match create_new_effect_draft_dir() {
                            Ok(dir) => dir,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                                continue;
                            }
                        };
                        let file_path = temp_dir.join("dsp.lisp");
                        if let Err(error) =
                            std::fs::write(&file_path, sequencer::lisp_host::EFFECT_TEMPLATE)
                        {
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write starter effect: {error}"
                            )));
                            continue;
                        }
                        match sequencer::lisp_host::compile_and_load_with_origin(
                            sequencer::lisp_host::EFFECT_TEMPLATE,
                            app.graph.sample_rate,
                            file_path.parent(),
                            sequencer::lisp_host::DGenSourceOrigin::Draft,
                        )
                        .and_then(|result| {
                            app.apply_compiled_effect_to_slot_sync(
                                result,
                                NEW_EFFECT_DRAFT_NAME,
                                slot,
                                track,
                            )
                        }) {
                            Ok(()) => {}
                            Err(error) => {
                                let _ = std::fs::remove_dir_all(&temp_dir);
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to create draft effect: {error}"
                                )));
                                continue;
                            }
                        }

                        let buf_name = "*effect-patcher:new-effect*".to_string();
                        editor.remove_buffer_by_name(&buf_name);
                        editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                        let patcher_source = effect_patcher_buffer_source(&buf_name, &file_path);
                        if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                            let _ = app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to build patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        reset_effect_patcher_state(&file_path);
                        let layout_source = show_instrument_patcher_layout_source(&buf_name);
                        if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                            let _ = app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        editor_buffer_name = Some(buf_name.clone());
                        editor_mode = Some("new-effect".to_string());
                        effect_edit_session = Some(EffectEditSession::begin_create_draft(
                            NEW_EFFECT_DRAFT_NAME.to_string(),
                            file_path,
                            buf_name.clone(),
                            EffectEditTarget::Track { track, slot },
                            sequencer::lisp_host::EFFECT_TEMPLATE.to_string(),
                            temp_dir,
                        ));
                        let rt = editor.runtime_mut();
                        let _ = rt.eval_str("(set! sbrowser-editor-name \"\")");
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String("new-effect".to_string()),
                        );
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(buf_name.clone()),
                        );
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
                        rt.run_reactive_cycle();
                        if let Err(error) = rt.eval_str("(sbrowser-refresh-buffer)") {
                            let _ = app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot);
                            if let Some(EffectEditSession {
                                mode: EffectEditMode::CreateDraft { temp_dir },
                                ..
                            }) = effect_edit_session.take()
                            {
                                let _ = std::fs::remove_dir_all(temp_dir);
                            }
                            editor.remove_buffer_by_name(&buf_name);
                            editor_buffer_name = None;
                            editor_mode = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                            rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive(
                                "SEQ",
                                "editor-buffer-name",
                                Value::String(String::new()),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to refresh effect editor sidebar: {error:?}"
                            )));
                            continue;
                        }
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*samples*");
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Created draft effect in slot {}",
                            slot + 1
                        )));
                    }

                    "save-new-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.trim().to_string();
                                    if effect_name.is_empty() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String("Name cannot be empty".to_string()),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let Some(session) = effect_edit_session.as_ref() else {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "No draft effect session is active".to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    };
                                    if !matches!(&session.mode, EffectEditMode::CreateDraft { .. })
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Current editor session is not a draft effect"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if !session.visible_revision_valid {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Cannot finalize: the current patch has errors"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let flushed_macros =
                                        match flush_staged_effect_library_macro_edits(session) {
                                            Ok(macros) => macros,
                                            Err(error) => {
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "editor-error",
                                                    Value::String(format!(
                                                        "Failed to save library macro edits: {error}"
                                                    )),
                                                );
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                continue;
                                            }
                                        };

                                    let final_slug =
                                        sequencer::agent::actions::normalize_patch_name(
                                            &effect_name,
                                            "new-effect",
                                        );
                                    let final_name = format!("{final_slug}/");
                                    let (final_dir, legacy_file) =
                                        finalized_effect_storage_paths(&final_slug);
                                    if final_dir.exists() || legacy_file.exists() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Effect '{final_slug}' already exists"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let source = session.last_valid_source.clone();
                                    if let Err(e) =
                                        sequencer::lisp_host::save_effect(&final_name, &source)
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("Failed to save: {e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let final_dsp =
                                        sequencer::lisp_host::effect_source_path(&final_name);
                                    if let Some(layout) = session.last_valid_layout.as_deref() {
                                        if let Err(e) =
                                            write_patcher_layout_sidecar(&final_dsp, layout)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save layout: {e}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    } else if let EffectEditMode::CreateDraft { temp_dir } =
                                        &session.mode
                                    {
                                        let source_dsp = temp_dir.join("dsp.lisp");
                                        if let Err(e) =
                                            copy_patcher_layout_sidecar(&source_dsp, &final_dsp)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save layout: {e}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    }
                                    let (track, slot) = match session.target {
                                        EffectEditTarget::Track { track, slot } => (track, slot),
                                        EffectEditTarget::Bus { .. } => {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            editor.handle_host_event(HostEvent::Error(
                                                "Draft effects can only target track effect slots"
                                                    .to_string(),
                                            ));
                                            continue;
                                        }
                                    };
                                    if let Err(error) =
                                        app.load_saved_effect_to_slot_sync(track, slot, &final_name)
                                    {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to load finalized effect: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let session =
                                        effect_edit_session.take().expect("session exists");
                                    if let EffectEditMode::CreateDraft { temp_dir } = session.mode {
                                        let _ = std::fs::remove_dir_all(temp_dir);
                                    }
                                    reset_effect_patcher_state(&session.path);
                                    if let Err(error) = editor
                                        .runtime_mut()
                                        .eval_str(restore_instrument_patcher_layout_source())
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to restore main editor layout: {error:?}"
                                        )));
                                        continue;
                                    }
                                    editor.refresh_runtime_side_effects();
                                    editor.remove_buffer_by_name(&session.buffer_name);
                                    if let Some(status) =
                                        staged_library_macro_flush_status(&flushed_macros)
                                    {
                                        editor.handle_host_event(HostEvent::Status(status));
                                    }
                                    editor_buffer_name = None;
                                    editor_mode = None;

                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "available-builtin-effects",
                                        build_available_builtin_effects(),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "available-effects",
                                        build_available_effects(),
                                    );
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
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Finalized effect '{}' in slot {}",
                                        display_instrument_name(&final_name),
                                        slot + 1
                                    )));
                                }
                            }
                        }
                    }

                    "enter-edit-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    let slot_idx =
                                        map.get("slot").and_then(|cell| match &*cell.borrow() {
                                            Value::Number(n) => Some(*n as usize),
                                            _ => None,
                                        });
                                    let bus_idx =
                                        map.get("bus").and_then(|cell| match &*cell.borrow() {
                                            Value::Number(n) => Some(*n as usize),
                                            _ => None,
                                        });
                                    let file_path =
                                        sequencer::lisp_host::effect_source_path(&effect_name);
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Effect file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let target = match (bus_idx, slot_idx) {
                                        (Some(bus), Some(slot)) => {
                                            EffectEditTarget::Bus { bus, slot }
                                        }
                                        (None, Some(slot)) => EffectEditTarget::Track {
                                            track: current_track.load(Ordering::Relaxed),
                                            slot,
                                        },
                                        _ => {
                                            editor.handle_host_event(HostEvent::Error(
                                                "Effect edit command did not include a target slot"
                                                    .to_string(),
                                            ));
                                            continue;
                                        }
                                    };
                                    let persisted_source = match std::fs::read_to_string(&file_path)
                                    {
                                        Ok(source) => source,
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to read '{}': {error}",
                                                file_path.display()
                                            )));
                                            continue;
                                        }
                                    };
                                    let buf_name = format!("*effect-patcher:{effect_name}*");
                                    editor.remove_buffer_by_name(&buf_name);
                                    editor.create_scratch_buffer(
                                        &buf_name,
                                        "",
                                        BufferMode::ESeqLisp,
                                    );
                                    let patcher_source =
                                        effect_patcher_buffer_source(&buf_name, &file_path);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&patcher_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to build patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    reset_effect_patcher_state(&file_path);
                                    let layout_source =
                                        show_instrument_patcher_layout_source(&buf_name);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&layout_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to show patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-effect".to_string());
                                    effect_edit_session =
                                        Some(EffectEditSession::begin_edit_existing(
                                            effect_name.clone(),
                                            file_path,
                                            buf_name.clone(),
                                            target,
                                            persisted_source,
                                        ));
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String("edit-effect".to_string()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(buf_name),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }

                    "update-effect" => {
                        let Some(session) = effect_edit_session.as_ref() else {
                            editor.handle_host_event(HostEvent::Error(
                                "No effect being edited".to_string(),
                            ));
                            continue;
                        };
                        if !session.visible_revision_valid {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Cannot save: the current patch has errors".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let flushed_macros = match flush_staged_effect_library_macro_edits(session)
                        {
                            Ok(macros) => macros,
                            Err(error) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!(
                                        "Failed to save library macro edits: {error}"
                                    )),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                continue;
                            }
                        };
                        if let Err(e) = std::fs::write(&session.path, &session.last_valid_source) {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Failed to save: {e}")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        if let Some(layout) = session.last_valid_layout.as_deref() {
                            if let Err(e) = write_patcher_layout_sidecar(&session.path, layout) {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("Failed to save layout: {e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                continue;
                            }
                        }
                        let session = effect_edit_session.take().expect("session exists");
                        reset_effect_patcher_state(&session.path);
                        if let Err(error) = editor
                            .runtime_mut()
                            .eval_str(restore_instrument_patcher_layout_source())
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to restore main editor layout: {error:?}"
                            )));
                            effect_edit_session = Some(session);
                            continue;
                        }
                        editor.refresh_runtime_side_effects();
                        editor.remove_buffer_by_name(&session.buffer_name);
                        if let Some(status) = staged_library_macro_flush_status(&flushed_macros) {
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                        editor_buffer_name = None;
                        editor_mode = None;

                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
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
                            "Saved effect '{}'",
                            session.name
                        )));
                    }

                    "cancel-editor" => {
                        if pending_instrument_cancel_restore.is_some()
                            || pending_effect_cancel_restore.is_some()
                        {
                            continue;
                        }
                        let cancelled_patcher =
                            instrument_edit_session.is_some() || effect_edit_session.is_some();
                        if let Some(session) = instrument_edit_session.take() {
                            pending_instrument_preview = None;
                            reset_instrument_patcher_state(&session.path);
                            match session.mode.clone() {
                                InstrumentEditMode::EditExisting { persisted_source } => {
                                    let source = persisted_source.clone();
                                    let sample_rate = app.graph.sample_rate;
                                    let asset_base =
                                        session.path.parent().map(|parent| parent.to_path_buf());
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    std::thread::spawn(move || {
                                        let result = sequencer::lisp_host::compile_and_load_instrument_with_asset_base(
                                            &source,
                                            sample_rate,
                                            asset_base.as_deref(),
                                        );
                                        let _ = tx.send(result);
                                    });
                                    pending_instrument_cancel_restore =
                                        Some(PendingInstrumentCancelRestore {
                                            session,
                                            persisted_source,
                                            receiver: rx,
                                        });
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.mark_needs_redraw();
                                    continue;
                                }
                                InstrumentEditMode::CreateDraft {
                                    temp_dir,
                                    draft_track,
                                    original_track,
                                } => {
                                    let delete_result = if app.tracks.len() > 1 {
                                        app.graph_controller().delete_track(draft_track)
                                    } else {
                                        app.graph_controller().clear_track_in_place(draft_track)
                                    };
                                    match delete_result {
                                        Ok(_) => {
                                            let restored_track = if app.tracks.is_empty() {
                                                0
                                            } else {
                                                original_track.min(app.tracks.len() - 1)
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
                                            *record_armed.lock().unwrap() =
                                                app.graph.record_armed.clone();
                                            let rt = editor.runtime_mut();
                                            sync_track_topology_state(
                                                rt,
                                                &app,
                                                &state,
                                                &mut track_names,
                                                restored_track,
                                                &selected_steps,
                                                &piano_roll_selection,
                                                &accumulator_names,
                                                &record_armed,
                                                &cached_track_peak_levels,
                                            );
                                            rt.clear_subtree_effects_for_named_target(
                                                "*sequencer*",
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            refresh_visible_track_topology_layouts(&mut editor);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to remove draft instrument track: {error}"
                                            )));
                                        }
                                    }
                                    let _ = std::fs::remove_dir_all(temp_dir);
                                }
                            }
                        }
                        if let Some(session) = effect_edit_session.take() {
                            pending_effect_preview = None;
                            reset_effect_patcher_state(&session.path);
                            match session.mode.clone() {
                                EffectEditMode::EditExisting { persisted_source } => {
                                    let source = persisted_source.clone();
                                    let sample_rate = app.graph.sample_rate;
                                    let asset_base =
                                        session.path.parent().map(|parent| parent.to_path_buf());
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    std::thread::spawn(move || {
                                        let result =
                                            sequencer::lisp_host::compile_and_load_with_asset_base(
                                                &source,
                                                sample_rate,
                                                asset_base.as_deref(),
                                            );
                                        let _ = tx.send(result);
                                    });
                                    pending_effect_cancel_restore =
                                        Some(PendingEffectCancelRestore {
                                            session,
                                            receiver: rx,
                                        });
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.mark_needs_redraw();
                                    continue;
                                }
                                EffectEditMode::CreateDraft { temp_dir } => {
                                    if let EffectEditTarget::Track { track, slot } = session.target
                                    {
                                        match app
                                            .graph_controller()
                                            .delete_custom_effect_slot(track, slot)
                                        {
                                            Ok(()) => {
                                                let rt = editor.runtime_mut();
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
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                editor.refresh_visible_layouts_for_buffer_named(
                                                    "*fx*",
                                                );
                                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            }
                                            Err(error) => {
                                                editor.handle_host_event(HostEvent::Error(
                                                    format!(
                                                    "Failed to remove draft effect slot: {error}"
                                                ),
                                                ));
                                            }
                                        }
                                    }
                                    let _ = std::fs::remove_dir_all(temp_dir);
                                }
                            }
                        }
                        if let Some(buf_name) = editor_buffer_name.take() {
                            if cancelled_patcher {
                                if let Err(error) = editor
                                    .runtime_mut()
                                    .eval_str(restore_instrument_patcher_layout_source())
                                {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Failed to restore main editor layout: {error:?}"
                                    )));
                                }
                                editor.refresh_runtime_side_effects();
                            } else {
                                editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                            }
                            editor.remove_buffer_by_name(&buf_name);
                        }

                        editor_mode = None;
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "editor-instrument-run-mode",
                            Value::String("instrument".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.handle_host_event(HostEvent::Status("Editor cancelled".to_string()));
                    }

                    other => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Unknown host command: {other}"
                        )));
                    }
                }
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
                        prev_recording = false;
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
                        if last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                            cached_cpu_load_bits =
                                state.transport.cpu_load_pct.load(Ordering::Relaxed);
                            last_cpu_ui_poll_at = Instant::now();
                        }
                        let cpu_load_pct = f32::from_bits(cached_cpu_load_bits);
                        let playing = state.transport.playing.load(Ordering::Relaxed);
                        let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        cached_peak_l_level = meter_display_level(f32::from_bits(
                            state.transport.peak_l.load(Ordering::Relaxed),
                        ));
                        cached_peak_r_level = meter_display_level(f32::from_bits(
                            state.transport.peak_r.load(Ordering::Relaxed),
                        ));
                        cached_track_peak_levels =
                            read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                        cached_bus_peak_levels =
                            read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                        (cached_modulator_phases, cached_modulator_levels) =
                            read_modulator_display_values(app.graph.lg, &app);
                        last_meter_poll_at = Instant::now();
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
                        rt.set_reactive("SEQ", "master-peak-l", Value::Number(cached_peak_l_level));
                        rt.set_reactive("SEQ", "master-peak-r", Value::Number(cached_peak_r_level));
                        rt.set_reactive(
                            "SEQ",
                            "master-recording",
                            Value::Bool(master_recording.load(Ordering::Acquire)),
                        );
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        sync_modulator_phase_fields(rt, &cached_modulator_phases);
                        sync_modulator_level_fields(rt, &cached_modulator_levels);
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
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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

                        prev_current_track = ct;
                        prev_playhead = playhead;
                        prev_transport_playhead = transport_playhead;
                        prev_bpm = bpm;
                        prev_playing = playing;
                        prev_pattern_epoch = epoch;
                        prev_cpu_load_bits = cached_cpu_load_bits;
                        prev_peak_l_level = cached_peak_l_level;
                        prev_peak_r_level = cached_peak_r_level;
                        prev_master_recording = master_recording.load(Ordering::Acquire);
                        prev_track_peak_levels = cached_track_peak_levels.clone();
                        prev_modulator_phases = cached_modulator_phases.clone();
                        prev_modulator_levels = cached_modulator_levels.clone();
                        prev_bus_playheads = bus_playhead_snapshot(&app);
                        prev_track_playheads = track_playheads_snapshot(&state, &app);
                        prev_track_button_states = track_button_state_snapshot(&state);
                        prev_ui_epoch = ui_epoch.load(Ordering::Relaxed);

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
        if pointer_released_this_loop && rack_control_snapshot_dirty {
            state.publish_scheduler_snapshot();
            rack_control_snapshot_dirty = false;
        }
        if pointer_released_this_loop {
            app::edit::finish_active_gesture(&mut app);
        } else if !pointer_is_down {
            app::edit::finish_active_gesture_if_idle(&mut app);
        }
        ui_loop_stats.note_host_commands(host_commands_started.elapsed());

        if let Some(completed_load) = pending_saved_instrument_load.as_ref().and_then(|pending| {
            match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Instrument load compile thread crashed".to_string()))
                }
            }
        }) {
            let pending = pending_saved_instrument_load
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
                                cached_track_peak_levels: &cached_track_peak_levels,
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
            pending_instrument_cancel_restore
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Instrument restore compile thread crashed".to_string()))
                    }
                })
        {
            let pending = pending_instrument_cancel_restore
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
                        if let Some(buf_name) = editor_buffer_name.take() {
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
                        editor_mode = None;
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
                        instrument_edit_session = Some(session);
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
                    instrument_edit_session = Some(session);
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
            pending_effect_cancel_restore.as_ref().and_then(|pending| {
                match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Effect restore compile thread crashed".to_string()))
                    }
                }
            })
        {
            let pending = pending_effect_cancel_restore
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
                            if let Some(buf_name) = editor_buffer_name.take() {
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
                            editor_mode = None;
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
                            effect_edit_session = Some(session);
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
                    effect_edit_session = Some(session);
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

        if let Some(completed_preview) = pending_instrument_preview.as_ref().and_then(|pending| {
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
            let _ = pending_instrument_preview.take();
            match completed_preview {
                Ok((generation, source, layout, compile_result)) => {
                    if let Some(session) = instrument_edit_session.as_mut() {
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
                    if let Some(session) = instrument_edit_session.as_mut() {
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
            pending_effect_preview
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
            let _ = pending_effect_preview.take();
            match completed_preview {
                Ok((generation, source, layout, compile_result)) => {
                    if let Some(session) = effect_edit_session.as_mut() {
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
                    if let Some(session) = effect_edit_session.as_mut() {
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
        for (key, pending) in &pending_agentic_bubbles {
            match pending.receiver.try_recv() {
                Ok(result) => completed_agentic.push((key.clone(), result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => completed_agentic
                    .push((key.clone(), Err("request worker disconnected".to_string()))),
            }
        }
        for (key, result) in completed_agentic {
            let Some(pending) = pending_agentic_bubbles.remove(&key) else {
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
            &mut watched_sampler_voice_track,
            &mut watched_sampler_voice_ids,
        );
        let reactive_sync_started = Instant::now();
        {
            let playing = state.transport.playing.load(Ordering::Relaxed);
            let bpm = state.transport.bpm.load(Ordering::Relaxed);
            if last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                cached_cpu_load_bits = state.transport.cpu_load_pct.load(Ordering::Relaxed);
                last_cpu_ui_poll_at = Instant::now();
            }
            let cpu_load_bits = cached_cpu_load_bits;
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
            let current_track_playhead_changed = playhead != prev_playhead;
            if last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL {
                cached_peak_l_level = meter_display_level(f32::from_bits(
                    state.transport.peak_l.load(Ordering::Relaxed),
                ));
                cached_peak_r_level = meter_display_level(f32::from_bits(
                    state.transport.peak_r.load(Ordering::Relaxed),
                ));
                cached_track_peak_levels =
                    read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                cached_rack_slot_peak_levels = read_rack_slot_peak_levels(app.graph.lg, &app);
                cached_bus_peak_levels =
                    read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                (cached_modulator_phases, cached_modulator_levels) =
                    read_modulator_display_values(app.graph.lg, &app);
                last_meter_poll_at = Instant::now();
            }
            let mut needs_reactive_cycle = false;
            let mut refresh_visible_step_after_cycle = false;
            let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
            if fx_visible {
                let active_notes = state.active_notes(ct);
                if active_notes != prev_instrument_active_notes {
                    needs_reactive_cycle |= editor
                        .runtime_mut()
                        .set_reactive(
                            "SEQ",
                            "instrument-active-notes",
                            build_active_notes_value(&active_notes),
                        )
                        .effects_dirty;
                    prev_instrument_active_notes = active_notes;
                }
            }
            if selected_neural_snapshot != prev_selected_neural_neurons {
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
                prev_selected_neural_neurons = selected_neural_snapshot.clone();
            }
            // Track switch — rebuild everything
            if ct != prev_current_track && !app.tracks.is_empty() {
                editor.reset_widget_scroll_for_buffer_named("*metal*");
                editor.reset_widget_scroll_for_buffer_named("*fx*");
                preview_plock_variant = None;
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
                sync_track_peak_fields(rt, &cached_track_peak_levels);
                sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                sync_modulator_phase_fields(rt, &cached_modulator_phases);
                sync_modulator_level_fields(rt, &cached_modulator_levels);
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
                prev_current_track = ct;
                prev_playhead = playhead;
                prev_transport_playhead = transport_playhead;
                prev_pattern_epoch = epoch;
                needs_reactive_cycle = true;
            }

            // Track-groups reconcile: pull native-mutated groups (collapse toggle,
            // group create) into app.groups and rebuild the SEQ.groups reactive.
            {
                let groups_snapshot = track_groups.lock().unwrap().clone();
                if groups_snapshot != prev_groups {
                    app.groups = groups_snapshot.clone();
                    let rt = editor.runtime_mut();
                    sync_groups_bindings(rt, &app.groups);
                    prev_groups = groups_snapshot;
                    needs_reactive_cycle = true;
                }
            }

            // Multi-select highlight reconcile. Runs after the track-switch block
            // so it overrides the single-select bindings written there.
            {
                let selected_snapshot = selected_tracks.lock().unwrap().clone();
                if selected_snapshot != prev_selected_tracks {
                    let rt = editor.runtime_mut();
                    sync_selected_tracks_bindings(rt, app.tracks.len(), ct, &selected_snapshot);
                    prev_selected_tracks = selected_snapshot;
                    needs_reactive_cycle = true;
                }
            }

            if playing != prev_playing {
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "playing", Value::Bool(playing));
                if sequencer_visible {
                    if playing {
                        sync_all_track_playhead_fields(rt, &state, &app);
                    } else {
                        clear_all_track_playhead_fields(rt, &app);
                    }
                }
                prev_playing = playing;
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
                    if preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                        *track != ct || !selected_steps.lock().unwrap().is_empty()
                    }) {
                        preview_plock_variant = None;
                    }
                    let preview_dirty = sync_track_plock_variant_preview(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        preview_plock_variant.as_ref(),
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
            if bpm != prev_bpm {
                app.push_all_delay_bpm();
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                prev_bpm = bpm;
                needs_reactive_cycle = true;
            }
            if transport_visible && cpu_load_bits != prev_cpu_load_bits {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "cpu-load-pct",
                        Value::Number(f32::from_bits(cpu_load_bits) as f64),
                    )
                    .effects_dirty;
                prev_cpu_load_bits = cpu_load_bits;
            }
            if !transport_visible && cpu_load_bits != prev_cpu_load_bits {
                prev_cpu_load_bits = cpu_load_bits;
            }
            let master_rec_on = master_recording.load(Ordering::Acquire);
            app.ui.master_recording = master_rec_on;
            if transport_visible && master_rec_on != prev_master_recording {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on))
                    .effects_dirty;
                prev_master_recording = master_rec_on;
            }
            if !transport_visible && master_rec_on != prev_master_recording {
                prev_master_recording = master_rec_on;
            }
            if master_meter_visible && cached_peak_l_level != prev_peak_l_level {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-peak-l", Value::Number(cached_peak_l_level))
                    .effects_dirty;
                prev_peak_l_level = cached_peak_l_level;
            }
            if !master_meter_visible && cached_peak_l_level != prev_peak_l_level {
                prev_peak_l_level = cached_peak_l_level;
            }
            if master_meter_visible && cached_peak_r_level != prev_peak_r_level {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-peak-r", Value::Number(cached_peak_r_level))
                    .effects_dirty;
                prev_peak_r_level = cached_peak_r_level;
            }
            if !master_meter_visible && cached_peak_r_level != prev_peak_r_level {
                prev_peak_r_level = cached_peak_r_level;
            }
            if cached_track_peak_levels != prev_track_peak_levels {
                if track_meter_visible {
                    needs_reactive_cycle |= sync_track_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_track_peak_levels,
                        &cached_track_peak_levels,
                    );
                }
                prev_track_peak_levels = cached_track_peak_levels.clone();
            }
            if cached_rack_slot_peak_levels != prev_rack_slot_peak_levels {
                if track_meter_visible {
                    needs_reactive_cycle |= sync_rack_slot_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_rack_slot_peak_levels,
                        &cached_rack_slot_peak_levels,
                    );
                }
                prev_rack_slot_peak_levels = cached_rack_slot_peak_levels.clone();
            }
            if cached_bus_peak_levels != prev_bus_peak_levels {
                if mixer_visible {
                    needs_reactive_cycle |= sync_bus_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_bus_peak_levels,
                        &cached_bus_peak_levels,
                    );
                }
                prev_bus_peak_levels = cached_bus_peak_levels.clone();
            }
            if last_neural_visualization_poll_at.elapsed() >= NEURAL_VISUALIZATION_POLL_INTERVAL {
                last_neural_visualization_poll_at = Instant::now();
                needs_reactive_cycle |=
                    sync_neural_visualization_fields(editor.runtime_mut(), &state);
            }
            if cached_modulator_phases != prev_modulator_phases {
                if fx_visible {
                    needs_reactive_cycle |= sync_modulator_phase_field_delta(
                        editor.runtime_mut(),
                        &prev_modulator_phases,
                        &cached_modulator_phases,
                    );
                }
                prev_modulator_phases = cached_modulator_phases.clone();
            }
            if cached_modulator_levels != prev_modulator_levels {
                if fx_visible {
                    needs_reactive_cycle |= sync_modulator_level_field_delta(
                        editor.runtime_mut(),
                        &prev_modulator_levels,
                        &cached_modulator_levels,
                    );
                }
                prev_modulator_levels = cached_modulator_levels.clone();
            }
            if bus_playheads != prev_bus_playheads {
                if metal_visible {
                    editor.runtime_mut().set_reactive(
                        "SEQ",
                        "bus-playheads",
                        build_bus_playheads_value(&app),
                    );
                    needs_reactive_cycle = true;
                }
                prev_bus_playheads = bus_playheads;
            }
            if sequencer_visible {
                let previous_track_playheads = prev_track_playheads.clone();
                if sync_track_playhead_field_delta(
                    editor.runtime_mut(),
                    &state,
                    &app,
                    &mut prev_track_playheads,
                ) {
                    needs_reactive_cycle = true;
                }
                if previous_track_playheads != prev_track_playheads {
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
                prev_track_playheads = track_playheads_snapshot(&state, &app);
            }
            if current_track_playhead_visible
                && (!prev_current_track_playhead_visible || playhead != prev_playhead)
                && !app.tracks.is_empty()
            {
                if prev_current_track_playhead_visible {
                    needs_reactive_cycle |= sync_playhead_field_delta(
                        editor.runtime_mut(),
                        prev_playhead as usize,
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
                prev_playhead = playhead;
            }
            if !current_track_playhead_visible && prev_playhead != playhead {
                prev_playhead = playhead;
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
                if preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                    *track != ct || !selected_steps.lock().unwrap().is_empty()
                }) {
                    preview_plock_variant = None;
                }
                let preview_dirty = sync_track_plock_variant_preview(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    preview_plock_variant.as_ref(),
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
            prev_current_track_playhead_visible = current_track_playhead_visible;
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
                    cached_track_peak_levels: &cached_track_peak_levels,
                    cached_bus_peak_levels: &cached_bus_peak_levels,
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
            if epoch != prev_pattern_epoch && !app.tracks.is_empty() {
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
                let old_pattern_epoch = prev_pattern_epoch;
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
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                }
                if mixer_visible {
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                if preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                    *track != ct || !selected_steps.lock().unwrap().is_empty()
                }) {
                    preview_plock_variant = None;
                }
                let preview_dirty = sync_track_plock_variant_preview(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    preview_plock_variant.as_ref(),
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
                prev_selected_neural_neurons = selected_neural_snapshot;
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
                prev_pattern_epoch = epoch;
                prev_track_button_states = track_button_state_snapshot(&state);
                needs_reactive_cycle = true;
                refresh_visible_mixer_after_cycle |= mixer_visible;
                profile_pattern_reactive_cycle = profile_switch;
            }
            let ui_ep = ui_epoch.load(Ordering::Relaxed);
            if ui_ep != prev_ui_epoch {
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] ui_epoch {}->{} visible metal={} mixer={} sequencer={} fx={} ct={}",
                        prev_ui_epoch,
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
                let track_buttons_changed = track_button_states != prev_track_button_states;
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] track_buttons_changed={} prev_buttons={} next_buttons={}",
                        track_buttons_changed,
                        prev_track_button_states.len(),
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
                        &cached_track_peak_levels,
                    );
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                    *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                    sync_track_params_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                    if preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                        *track != ct || !selected_steps.lock().unwrap().is_empty()
                    }) {
                        preview_plock_variant = None;
                    }
                    let preview_dirty = sync_track_plock_variant_preview(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        preview_plock_variant.as_ref(),
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
                prev_master_recording = master_rec_on;
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
                prev_track_button_states = track_button_states;
                prev_ui_epoch = ui_ep;
                needs_reactive_cycle = true;
            }
            let fx_ep = fx_epoch.load(Ordering::Relaxed);
            if fx_visible && fx_ep != prev_fx_epoch {
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
                prev_fx_epoch = fx_ep;
                needs_reactive_cycle = true;
            }
            if transport_visible && transport_playhead != prev_transport_playhead {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "transport-playhead",
                        Value::Number(transport_playhead as f64),
                    )
                    .effects_dirty;
                prev_transport_playhead = transport_playhead;
            }
            if !transport_visible && transport_playhead != prev_transport_playhead {
                prev_transport_playhead = transport_playhead;
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
                if analysis_key != prev_sampler_analysis_key {
                    if let Some((ct, _, _, _, _)) = analysis_key {
                        app.publish_sampler_analysis_runtime(ct);
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(&app, ct, &selected_steps),
                        );
                        needs_reactive_cycle = true;
                    }
                    prev_sampler_analysis_key = analysis_key;
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
            if auto_follow != prev_auto_follow {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "auto-follow", Value::Bool(auto_follow));
                prev_auto_follow = auto_follow;
                needs_reactive_cycle = true;
            }
            let editor_macro_action = instrument_edit_session
                .as_ref()
                .and_then(active_instrument_editor_macro_action)
                .or_else(|| {
                    effect_edit_session
                        .as_ref()
                        .and_then(active_effect_editor_macro_action)
                });
            let editor_macro_action = editor_macro_action_strings(editor_macro_action.as_ref());
            if editor_macro_action != prev_editor_macro_action {
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
                prev_editor_macro_action = editor_macro_action;
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
