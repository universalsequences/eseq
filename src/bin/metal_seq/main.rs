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
mod constants;
mod custom_ui;
mod editor_setup;
mod host_commands;
mod input;
mod natives;
mod piano_roll;
mod profile;
mod sampler_monitor;
mod state_values;
mod values;

use browser::*;
use constants::*;
use custom_ui::*;
use editor_setup::*;
use host_commands::*;
use input::*;
use natives::*;
use piano_roll::*;
use profile::*;
use sampler_monitor::*;
use state_values::*;
use values::*;

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::Event;

use eseqlisp::backend::Backend;
use eseqlisp::editor::ViewMode;
use eseqlisp::vm::Value;
use eseqlisp::{BufferMode, Editor, HostCommand, HostEvent, Runtime};

use sequencer::engine;
use sequencer::sequencer::{
    KeyboardTrigger, MidiFxPosition, SequencerState, StepParam, SwingResolution, Timebase,
    TrackOutput, TrackSendSnapshot, MAX_STEPS, SYNC_RESOLUTIONS,
};
use sequencer::ui;
use std::sync::atomic::AtomicBool;

fn pull_shared_bus_state(
    app: &mut ui::App,
    bus_state: &Arc<Mutex<Vec<ui::BusChannelState>>>,
) -> bool {
    let latest = bus_state.lock().unwrap().clone();
    if app.buses.len() != latest.len()
        || app
            .buses
            .iter()
            .zip(latest.iter())
            .any(|(a, b)| a.volume != b.volume || a.mute != b.mute || a.solo != b.solo)
    {
        if app.buses.len() == latest.len() {
            for (bus, latest_bus) in app.buses.iter_mut().zip(latest.iter()) {
                bus.volume = latest_bus.volume;
                bus.mute = latest_bus.mute;
                bus.solo = latest_bus.solo;
            }
        } else {
            app.buses = latest;
        }
        true
    } else {
        false
    }
}

fn editor_has_visible_buffer(editor: &Editor, name: &str) -> bool {
    editor.tile_root.leaf_ids().into_iter().any(|tile_id| {
        editor
            .tile_root
            .find_leaf(tile_id)
            .and_then(|leaf| editor.buffers.get(leaf.buffer_idx))
            .is_some_and(|buffer| buffer.name == name && buffer.view_mode != ViewMode::TextOnly)
    })
}

fn track_button_state_snapshot(state: &Arc<SequencerState>) -> Vec<(bool, bool)> {
    (0..state.active_track_count())
        .map(|track| {
            let params = &state.pattern.track_params[track];
            (params.is_muted(), params.is_solo())
        })
        .collect()
}

fn map_number(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<f64> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::Number(value) => Some(*value),
        _ => None,
    })
}

fn map_string(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<String> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    sequencer::crash::install()?;

    // 1. Init audio engine
    let eng = engine::init_engine()?;
    let lg_ptr = eng.lg_ptr;
    let state = eng.state.clone();
    let stream = eng._stream;

    // 2. Create App. Start intentionally empty so the first action is choosing
    // a sound instead of editing a canned pattern.
    let mut app = ui::App::new(
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
    let bus_state: Arc<Mutex<Vec<ui::BusChannelState>>> = Arc::new(Mutex::new(app.buses.clone()));
    let bus_node_ids: Arc<Mutex<Vec<ui::BusNodeIds>>> =
        Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
    let lg_raw = lg_ptr.0;

    // Shared current track index
    let current_track = Arc::new(AtomicUsize::new(0));
    // Selected steps for p-locking
    let selected_steps: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
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
    // When set, pagination stays on the user-selected page until the cooldown expires.
    let auto_follow_override_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    // Recording state shared between native functions and event loop
    let recording = Arc::new(AtomicBool::new(false));
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
    } = init_runtime(
        &app,
        state.clone(),
        &track_names,
        track_pan_ids.clone(),
        bus_state.clone(),
        bus_node_ids.clone(),
        current_track.clone(),
        selected_steps.clone(),
        piano_roll_selection.clone(),
        piano_roll_move_state.clone(),
        recording.clone(),
        record_armed.clone(),
        ui_epoch.clone(),
        fx_epoch.clone(),
        auto_follow_override_until.clone(),
        lg_raw,
    );

    let (mut editor, mut backend) = create_editor_and_backend(runtime, &app)?;

    // 5. Metal event loop
    let frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let mut last_render_at = Instant::now() - frame_interval;
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;
    let mut soft_step_param_edit = SoftStepParamEdit::default();

    // Inline editor session state (instrument/effect creation/editing)
    let mut editor_buffer_name: Option<String> = None;
    let mut editor_mode: Option<String> = None;
    let mut editor_effect_name: Option<String> = None; // original effect name (without .lisp)
    let mut editor_effect_slot: Option<usize> = None; // effect slot index for hot-swap
    let mut editor_effect_bus: Option<usize> = None; // bus index for bus effect hot-swap

    let mut prev_playing = false;
    let mut prev_bpm: u32 = 0;
    let mut prev_playhead: u32 = u32::MAX;
    let mut prev_transport_playhead: u32 = u32::MAX;
    let mut prev_pattern_epoch: u64 = 0;
    let mut prev_snapshot_version: u64 = 0;
    let mut prev_current_track: usize = usize::MAX;
    let mut prev_cpu_load_bits: u32 = u32::MAX;
    let mut prev_peak_l_level = -1.0f64;
    let mut prev_peak_r_level = -1.0f64;
    let mut prev_track_peak_levels: Vec<f64> = Vec::new();
    let mut prev_bus_peak_levels: Vec<f64> = Vec::new();
    let mut prev_bus_playheads: Vec<usize> = Vec::new();
    let mut prev_track_playheads: Vec<u32> = Vec::new();
    let mut prev_track_button_states = track_button_state_snapshot(&state);
    let mut prev_current_track_playhead_visible = false;
    let mut prev_ui_epoch: usize = 0;
    let mut prev_fx_epoch: usize = 0;
    let mut prev_auto_follow = true;
    let mut watched_sampler_voice_track: Option<usize> = None;
    let mut watched_sampler_voice_ids: Vec<i32> = Vec::new();
    let mut cached_peak_l_level = 0.0f64;
    let mut cached_peak_r_level = 0.0f64;
    let mut cached_track_peak_levels = vec![0.0; track_names.len()];
    let mut cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
    let mut last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL;
    let mut last_cpu_ui_poll_at = Instant::now() - CPU_UI_POLL_INTERVAL;
    let mut last_voice_count_log_at = Instant::now() - VOICE_COUNT_LOG_INTERVAL;
    let log_voice_counts = std::env::var_os("TINYSEQ_LOG_VOICE_COUNTS").is_some();
    let mut cached_cpu_load_bits: u32 = 0.0f32.to_bits();

    eprintln!("metal_seq: entering event loop");
    let mut ui_loop_stats = UiLoopStats::new();

    loop {
        pull_shared_bus_state(&mut app, &bus_state);
        pull_named_scratch_buffer_into_project(&editor, &mut app);
        editor.update_timers();
        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);
        if log_voice_counts && last_voice_count_log_at.elapsed() >= VOICE_COUNT_LOG_INTERVAL {
            log_active_voice_counts(&state, &track_names);
            last_voice_count_log_at = Instant::now();
        }

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
            backend
                .render_tiled(&tiled_frame)
                .map_err(|_| "render failed")?;
            let render_elapsed = render_started.elapsed();
            ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed);
            editor.clear_needs_redraw();
            last_render_at = Instant::now();
            continue;
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
        if let Some(event) = backend.poll_event(timeout) {
            let event_started = Instant::now();
            match event {
                Event::Key(raw_key) => {
                    if handle_metal_command_shortcut(
                        &mut editor,
                        &raw_key,
                        &state,
                        &current_track,
                        &selected_steps,
                        &step_clipboard,
                    ) {
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    let key = normalize_command_shortcuts(raw_key);
                    if should_toggle_play_on_space(&editor, &key) {
                        let _ = editor.runtime_mut().eval_str("(seq-toggle-play)");
                        editor.refresh_runtime_side_effects();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    if handle_metal_soft_step_param_key(
                        &mut editor,
                        &key,
                        &state,
                        &current_track,
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
                            &current_track,
                            &held_notes,
                            &ui_epoch,
                        )
                    } else {
                        RecordingKeyOutcome::Ignored
                    };
                    let intercepted = recording_key_outcome.consumed();
                    if recording_key_outcome.recorded() {
                        let ct = current_track.load(Ordering::Relaxed);
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                        sync_all_track_sequencer_state(rt, &state, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                        editor.mark_needs_redraw();
                    }
                    // Only pass Press events to the editor (Release is only for note-off)
                    if !intercepted && key.kind == crossterm::event::KeyEventKind::Press {
                        let should_reload_custom_ui = should_reload_custom_ui_after_key(&key);
                        editor.handle_key(key);
                        if should_reload_custom_ui {
                            reload_custom_instrument_ui(&mut editor);
                        }
                    }
                }
                Event::Mouse(mouse) => {
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
                        }
                        editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
                    }
                }
                Event::Resize(_, _) => editor.mark_needs_redraw(),
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
        }
        ui_loop_stats.note_gestures(gestures_started.elapsed());

        // 1b. Drain host commands (sample browser etc.)
        let host_commands_started = Instant::now();
        for command in editor.drain_host_commands() {
            if let HostCommand::Custom { name, payload } = command {
                match name.as_str() {
                    "audition-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        if let Some(path_str) = path_str {
                            if app.tracks.is_empty() {
                                editor.handle_host_event(HostEvent::Status(
                                    "Add a track before auditioning samples".to_string(),
                                ));
                                continue;
                            }
                            let path = Path::new(&path_str);
                            let track = current_track.load(Ordering::Relaxed);
                            match sequencer::sampler::load_wav_buffer(lg_raw, path) {
                                Ok((new_buffer_id, new_name)) => {
                                    register_waveform_sample(path);
                                    app.graph_controller()
                                        .send_buffer_to_all_voices(track, new_buffer_id);
                                    app.graph.track_buffer_ids[track] = new_buffer_id;
                                    app.tracks[track] = new_name.clone();
                                    app.register_sample_path(&new_name, path.to_path_buf());
                                    if track < app.sampler_paths.len() {
                                        app.sampler_paths[track] = Some(path.to_path_buf());
                                    }
                                    track_names[track] = new_name.clone();
                                    // Update reactive state
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&track_names),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, track, &selected_steps),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Audition: {new_name}"
                                    )));
                                }
                                Err(e) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error loading sample: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-sampler" => match app.graph_controller().add_blank_sampler_track() {
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
                            rt.set_reactive("SEQ", "current-track", Value::Number(idx as f64));
                            rt.set_reactive("SEQ", "track-names", build_track_names(&track_names));
                            sync_all_track_sequencer_state(rt, &state, &app);
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
                            sync_track_params(rt, &app, &state, idx, &selected_steps);
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
                    "add-track-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        if let Some(path_str) = path_str {
                            let path = Path::new(&path_str);
                            match app.graph_controller().add_track(path) {
                                Ok(idx) => {
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "current-track",
                                        Value::Number(idx as f64),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&track_names),
                                    );
                                    sync_all_track_sequencer_state(rt, &state, &app);
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
                                    *accumulator_names.lock().unwrap() =
                                        build_accumulator_names(&app);
                                    sync_track_params(rt, &app, &state, idx, &selected_steps);
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
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error adding track: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-instrument" => {
                        handle_add_track_instrument_command(
                            &payload,
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
                                ui_epoch: &ui_epoch,
                                lg_raw,
                            },
                        );
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

                        match app.graph_controller().delete_track(track) {
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
                                last_meter_poll_at = Instant::now();
                                *record_armed.lock().unwrap() = app.graph.record_armed.clone();

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
                                rt.clear_subtree_effects_for_named_target("*sequencer*");
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
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
                                match load_instrument_preset_into_track(
                                    &mut app,
                                    track,
                                    &preset_name,
                                ) {
                                    Ok(()) => {
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
                                    app.save_current_track_as_preset(&name, overwrite);
                                    // Refresh sidebar presets list
                                    let rt = editor.runtime_mut();
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Saved preset '{name}'"
                                    )));
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
                    "save-project" => {
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
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetInstrumentParam {
                                            track,
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
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentParam {
                                                track,
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
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetInstrumentPlockMulti {
                                            track,
                                            steps,
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
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentPlockMulti {
                                                track,
                                                steps,
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
                                        app.apply_effect_sidechain_selection(
                                            track,
                                            slot_idx,
                                            param_idx,
                                            selected_idx,
                                        );
                                        if let Some(slot) = app
                                            .state
                                            .pattern
                                            .effect_chains
                                            .get(track)
                                            .and_then(|chain| chain.get(slot_idx))
                                        {
                                            slot.defaults.set(param_idx, selected_idx as f32);
                                        }
                                        app.state.publish_scheduler_snapshot();
                                    } else {
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetEffectParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
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
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetTrackOutput { track, output },
                                    );
                                    let rt = editor.runtime_mut();
                                    sync_track_mixer_state(rt, &app, &state);
                                    if track == current_track.load(Ordering::Relaxed) {
                                        sync_track_params(rt, &app, &state, track, &selected_steps);
                                    }
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
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
                                let track = payload_track
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
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
                                ui::apply_command(
                                    &mut app,
                                    ui::AppCommand::SetTrackSends { track, sends },
                                );
                                let rt = editor.runtime_mut();
                                sync_track_mixer_state(rt, &app, &state);
                                if track == current_track.load(Ordering::Relaxed) {
                                    sync_track_params(rt, &app, &state, track, &selected_steps);
                                }
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                                match app.set_bus_effect_param(bus_idx, slot_idx, param_idx, value)
                                {
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
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    if let Some(slot) = bus.effect_slots.get_mut(slot_idx) {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        for step in steps {
                                            if step < slot.plocks.len()
                                                && param_idx < slot.plocks[step].len()
                                            {
                                                slot.plocks[step][param_idx] = Some(value);
                                            }
                                        }
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
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
                                    if is_host_sidechain {
                                        app.apply_bus_effect_sidechain_selection(
                                            bus_idx,
                                            slot_idx,
                                            param_idx,
                                            selected_idx,
                                        );
                                    }
                                    match app.set_bus_effect_param(
                                        bus_idx,
                                        slot_idx,
                                        param_idx,
                                        selected_idx as f32,
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
                                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                                        if let Some(slot) = bus.effect_slots.get_mut(slot_idx) {
                                            let steps: Vec<usize> = selected_steps
                                                .lock()
                                                .unwrap()
                                                .iter()
                                                .copied()
                                                .collect();
                                            for step in steps {
                                                if step < slot.plocks.len()
                                                    && param_idx < slot.plocks[step].len()
                                                {
                                                    slot.plocks[step][param_idx] =
                                                        Some(selected_idx as f32);
                                                }
                                            }
                                            app.publish_bus_gate_runtime();
                                            *bus_state.lock().unwrap() = app.buses.clone();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                                        app.apply_effect_sidechain_selection(
                                            track,
                                            slot_idx,
                                            param_idx,
                                            selected_idx,
                                        );
                                        if let Some(slot) = app
                                            .state
                                            .pattern
                                            .effect_chains
                                            .get(track)
                                            .and_then(|chain| chain.get(slot_idx))
                                        {
                                            slot.defaults.set(param_idx, selected_idx as f32);
                                        }
                                        app.state.publish_scheduler_snapshot();
                                    } else {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetEffectPlockMulti {
                                                track,
                                                slot_idx,
                                                steps,
                                                param_idx,
                                                value: selected_idx as f32,
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
                                let clamped = chain
                                    .get(slot_idx)
                                    .and_then(|name| {
                                        sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                    })
                                    .and_then(|desc| desc.params.get(param_idx).cloned())
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                if let Some(slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    slot.defaults.set(param_idx, clamped);
                                    state.publish_scheduler_snapshot();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                                        sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                    })
                                    .and_then(|desc| desc.params.get(param_idx).cloned())
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                if let Some(slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    for step in steps {
                                        slot.plocks.set(step, param_idx, clamped);
                                    }
                                    state.publish_scheduler_snapshot();
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
                                    if let Some(slot) = state
                                        .pattern
                                        .midi_fx_slots
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                    {
                                        slot.defaults.set(param_idx, selected_idx as f32);
                                        state.publish_scheduler_snapshot();
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
                                    if let Some(slot) = state
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
                                        for step in steps {
                                            slot.plocks.set(step, param_idx, selected_idx as f32);
                                        }
                                        state.publish_scheduler_snapshot();
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
                                ui::apply_command(
                                    &mut app,
                                    ui::AppCommand::SetInstrumentBaseNoteOffset {
                                        track,
                                        value: clamped,
                                    },
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                                        state.pattern.timebase_plocks[track]
                                            .set(step, sequencer::sequencer::Timebase::ALL[idx]);
                                        state.publish_scheduler_snapshot();
                                    }
                                    "swing" => {
                                        state.pattern.swing_plocks[track].set(step, value);
                                        state.publish_scheduler_snapshot();
                                    }
                                    "swing-resolution" => {
                                        let idx = (value.round() as usize).min(
                                            sequencer::sequencer::SwingResolution::ALL.len() - 1,
                                        );
                                        state.pattern.swing_resolution_plocks[track].set(
                                            step,
                                            sequencer::sequencer::SwingResolution::ALL[idx],
                                        );
                                        state.publish_scheduler_snapshot();
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
                                                state.pattern.instrument_slots[track]
                                                    .plocks
                                                    .set(step, param_idx, stored);
                                                state.publish_scheduler_snapshot();
                                            }
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
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
                                                slot.plocks.set(step, param_idx, clamped);
                                                state.publish_scheduler_snapshot();
                                            }
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
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
                                                        sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                                    })
                                                    .and_then(|desc| {
                                                        desc.params.get(param_idx).cloned()
                                                    })
                                                    .map(|p| value.clamp(p.min, p.max))
                                                    .unwrap_or(value);
                                                slot.plocks.set(step, param_idx, clamped);
                                                state.publish_scheduler_snapshot();
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
                                            state.pattern.timebase_plocks[track].set(
                                                step,
                                                sequencer::sequencer::Timebase::ALL[idx],
                                            );
                                            state.publish_scheduler_snapshot();
                                        }
                                    }
                                    "swing-resolution" => {
                                        if let Some(idx) =
                                            sequencer::sequencer::SwingResolution::LABELS
                                                .iter()
                                                .position(|item| *item == label)
                                        {
                                            state.pattern.swing_resolution_plocks[track].set(
                                                step,
                                                sequencer::sequencer::SwingResolution::ALL[idx],
                                            );
                                            state.publish_scheduler_snapshot();
                                        }
                                    }
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
                                                state.pattern.instrument_slots[track].plocks.set(
                                                    step,
                                                    param_idx,
                                                    selected_idx as f32,
                                                );
                                                state.publish_scheduler_snapshot();
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
                                                if let Some(slot) = state
                                                    .pattern
                                                    .effect_chains
                                                    .get(track)
                                                    .and_then(|chain| chain.get(slot_idx))
                                                {
                                                    slot.plocks.set(
                                                        step,
                                                        param_idx,
                                                        selected_idx as f32,
                                                    );
                                                    state.publish_scheduler_snapshot();
                                                }
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
                                                    sequencer::lisp_effect::load_midi_fx_descriptor(
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
                                                if let Some(slot) = state
                                                    .pattern
                                                    .midi_fx_slots
                                                    .get(track)
                                                    .and_then(|slots| slots.get(slot_idx))
                                                {
                                                    slot.plocks.set(
                                                        step,
                                                        param_idx,
                                                        selected_idx as f32,
                                                    );
                                                    state.publish_scheduler_snapshot();
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
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let (Some(target), Some(step)) = (target, step) {
                                let track = current_track.load(Ordering::Relaxed);
                                match target.as_str() {
                                    "timebase" => state.pattern.timebase_plocks[track].clear(step),
                                    "swing" => state.pattern.swing_plocks[track].clear(step),
                                    "swing-resolution" => {
                                        state.pattern.swing_resolution_plocks[track].clear(step)
                                    }
                                    "instrument" => {
                                        if let Some(param_idx) = param_idx {
                                            state.pattern.instrument_slots[track]
                                                .plocks
                                                .clear_param(step, param_idx);
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
                                                .pattern
                                                .effect_chains
                                                .get(track)
                                                .and_then(|chain| chain.get(slot_idx))
                                            {
                                                slot.plocks.clear_param(step, param_idx);
                                            }
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
                                                .pattern
                                                .midi_fx_slots
                                                .get(track)
                                                .and_then(|slots| slots.get(slot_idx))
                                            {
                                                slot.plocks.clear_param(step, param_idx);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                state.publish_scheduler_snapshot();
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
                                match app.add_bus_effect_sync(bus_idx, &effect_name) {
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
                                match app.add_builtin_bus_effect_sync(bus_idx, &effect_name) {
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
                    "add-builtin-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    let track = current_track.load(Ordering::Relaxed);
                                    app.ui.cursor_track = track;
                                    match app.add_builtin_effect_sync(track, &effect_name) {
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
                    "add-midi-fx" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(fx_name) = &*cell.borrow() {
                                    let fx_name = fx_name.clone();
                                    let track = current_track.load(Ordering::Relaxed);
                                    match app.add_midi_fx_to_track_sync(track, &fx_name) {
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
                            match app.delete_bus_effect_slot(bus_idx, slot_idx) {
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
                        match app
                            .graph_controller()
                            .delete_custom_effect_slot(track, slot_idx)
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
                        match app.delete_midi_fx_slot(track, slot_idx) {
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
                    "switch-pattern" => {
                        if let Value::Map(ref map) = payload {
                            let idx = map.get("idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            if let Some(idx) = idx {
                                let num_tracks = app.tracks.len();
                                let current_pattern =
                                    app.state.pattern.current_pattern.load(Ordering::Relaxed)
                                        as usize;
                                let num_patterns =
                                    app.state.pattern.num_patterns.load(Ordering::Relaxed) as usize;
                                if idx != current_pattern && idx < num_patterns {
                                    app.switch_bus_pattern(idx);
                                }
                                if let Some(sample_ids) = app.state.switch_pattern(
                                    idx,
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                ) {
                                    app.graph_controller().apply_sample_ids(&sample_ids);
                                    app.push_all_restored_defaults();
                                    let ct = current_track.load(Ordering::Relaxed);
                                    let rt = editor.runtime_mut();
                                    sync_track_name_state(rt, &mut track_names, &app);
                                    sync_pattern_state(rt, &state);
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                                    sync_all_track_sequencer_state(rt, &state, &app);
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
                                    *accumulator_names.lock().unwrap() =
                                        build_accumulator_names(&app);
                                    sync_track_params(rt, &app, &state, ct, &selected_steps);
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
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "propagate-current-track-to-all-patterns" => {
                        let track = match payload {
                            Value::Number(n) => n as usize,
                            _ => current_track.load(Ordering::Relaxed),
                        };
                        let num_patterns =
                            state.pattern.num_patterns.load(Ordering::Relaxed) as usize;
                        if track >= app.tracks.len() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track {} is out of range",
                                track + 1
                            )));
                        } else if num_patterns <= 1 {
                            editor.handle_host_event(HostEvent::Status(
                                "Nothing to propagate: only one pattern exists".to_string(),
                            ));
                        } else if app.state.propagate_track_to_all_patterns(
                            track,
                            app.tracks.len(),
                            &app.graph.track_buffer_ids,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
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
                        app.save_current_bus_pattern();
                        let source_pattern =
                            app.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
                        let new_idx = app.state.clone_pattern(
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        );
                        app.clone_bus_pattern_from_to(source_pattern, new_idx);
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
                        app.save_current_bus_pattern();
                        let deleted_pattern =
                            app.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
                        if let Some(sample_ids) = app.state.delete_pattern(
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            app.graph_controller().apply_sample_ids(&sample_ids);
                            app.push_all_restored_defaults();
                            let new_pattern =
                                app.state.pattern.current_pattern.load(Ordering::Relaxed) as usize;
                            app.delete_bus_pattern_at(deleted_pattern, new_pattern);
                            let ct = current_track.load(Ordering::Relaxed);
                            let rt = editor.runtime_mut();
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
                            sync_track_params(rt, &app, &state, ct, &selected_steps);
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
                    // ── Inline instrument/effect editor commands ──
                    "enter-new-instrument-editor" => {
                        let temp_path =
                            std::path::PathBuf::from("instruments/.untitled-instrument.lisp");
                        std::fs::create_dir_all("instruments").ok();
                        if let Err(e) =
                            std::fs::write(&temp_path, sequencer::lisp_effect::INSTRUMENT_TEMPLATE)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write template: {e}"
                            )));
                        } else {
                            let buf_name =
                                match editor.create_file_buffer(&temp_path, BufferMode::DGenLisp) {
                                    Ok(name) => name,
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to create buffer: {e:?}"
                                        )));
                                        continue;
                                    }
                                };
                            editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                            editor_buffer_name = Some(buf_name.clone());
                            editor_mode = Some("new-instrument".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                            rt.set_reactive(
                                "SEQ",
                                "editor-mode",
                                Value::String("new-instrument".to_string()),
                            );
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
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
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    // Write to final path
                                    let final_path = format!("instruments/{inst_name}.lisp");
                                    if let Err(e) = std::fs::write(&final_path, &source) {
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

                                    // Try to compile FIRST — stay in editor on failure
                                    match app.add_saved_instrument_track_sync(&inst_name) {
                                        Ok(idx) => {
                                            // Success — clean up temp file, close editor
                                            let _ = std::fs::remove_file(
                                                "instruments/.untitled-instrument.lisp",
                                            );
                                            editor
                                                .swap_buffer_in_tile_showing(&buf_name, "*metal*");
                                            editor.remove_buffer_by_name(&buf_name);
                                            editor_buffer_name = None;
                                            editor_mode = None;

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

                                            current_track.store(idx, Ordering::Relaxed);
                                            let new_name = app.tracks[idx].clone();
                                            track_names.push(new_name.clone());
                                            {
                                                let mut pan_ids = track_pan_ids.lock().unwrap();
                                                pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                                                push_solo_mutes(lg_raw, &state, &pan_ids);
                                            }
                                            record_armed.lock().unwrap().push(false);
                                            rt.set_reactive(
                                                "SEQ",
                                                "num-tracks",
                                                Value::Number(track_names.len() as f64),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-ids",
                                                build_track_ids(&app),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "current-track",
                                                Value::Number(idx as f64),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-names",
                                                build_track_names(&track_names),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "steps",
                                                build_steps_value(&state, idx),
                                            );
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
                                                build_midi_effects_value(
                                                    &state,
                                                    idx,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "instrument-panel",
                                                build_instrument_panel_value(
                                                    &app,
                                                    idx,
                                                    &selected_steps,
                                                ),
                                            );
                                            *accumulator_names.lock().unwrap() =
                                                build_accumulator_names(&app);
                                            sync_track_params(
                                                rt,
                                                &app,
                                                &state,
                                                idx,
                                                &selected_steps,
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
                                                "Created instrument '{inst_name}' and added track {}",
                                                idx + 1
                                            )));
                                        }
                                        Err(e) => {
                                            // Compile failed — stay in editor, show error
                                            // Clean up the written file so stale source doesn't linger
                                            let _ = std::fs::remove_file(&final_path);
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

                    "enter-edit-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    let file_path =
                                        match sequencer::lisp_effect::instrument_source_path(
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
                                    let buf_name = match editor
                                        .create_file_buffer(&file_path, BufferMode::DGenLisp)
                                    {
                                        Ok(name) => name,
                                        Err(e) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to open buffer: {e:?}"
                                            )));
                                            continue;
                                        }
                                    };
                                    editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-instrument".to_string());
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
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }

                    "update-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    if let Err(e) =
                                        sequencer::lisp_effect::save_instrument(&inst_name, &source)
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

                    "enter-new-effect-editor" => {
                        let temp_path = std::path::PathBuf::from("effects/.untitled-effect.lisp");
                        std::fs::create_dir_all("effects").ok();
                        if let Err(e) =
                            std::fs::write(&temp_path, sequencer::lisp_effect::EFFECT_TEMPLATE)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write template: {e}"
                            )));
                        } else {
                            let buf_name =
                                match editor.create_file_buffer(&temp_path, BufferMode::DGenLisp) {
                                    Ok(name) => name,
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to create buffer: {e:?}"
                                        )));
                                        continue;
                                    }
                                };
                            editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                            editor_buffer_name = Some(buf_name.clone());
                            editor_mode = Some("new-effect".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                            rt.set_reactive(
                                "SEQ",
                                "editor-mode",
                                Value::String("new-effect".to_string()),
                            );
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                        }
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
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    // Validate compilation before saving — stay in editor on failure
                                    let sr = app.graph.sample_rate;
                                    if let Err(e) =
                                        sequencer::lisp_effect::compile_and_load(&source, sr)
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("{e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let final_path = format!("effects/{effect_name}.lisp");
                                    if let Err(e) = std::fs::write(&final_path, &source) {
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
                                    let _ = std::fs::remove_file("effects/.untitled-effect.lisp");

                                    // Compilation validated — close editor
                                    editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                                    editor.remove_buffer_by_name(&buf_name);
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
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();

                                    // Add effect to current track (re-compiles from file, uses cache)
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
                                    let file_path = std::path::PathBuf::from(format!(
                                        "effects/{effect_name}.lisp"
                                    ));
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Effect file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let buf_name = match editor
                                        .create_file_buffer(&file_path, BufferMode::DGenLisp)
                                    {
                                        Ok(name) => name,
                                        Err(e) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to open buffer: {e:?}"
                                            )));
                                            continue;
                                        }
                                    };
                                    editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-effect".to_string());
                                    editor_effect_name = Some(effect_name.clone());
                                    editor_effect_slot = slot_idx;
                                    editor_effect_bus = bus_idx;
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
                        {
                            let effect_name = match editor_effect_name.clone() {
                                Some(n) => n,
                                None => {
                                    editor.handle_host_event(HostEvent::Error(
                                        "No effect being edited".to_string(),
                                    ));
                                    continue;
                                }
                            };
                            let buf_name = editor_buffer_name.clone().unwrap_or_default();
                            let source = editor.read_buffer_text(&buf_name).unwrap_or_default();

                            // Validate compilation before saving — stay in editor on failure
                            let sr = app.graph.sample_rate;
                            if let Err(e) = sequencer::lisp_effect::compile_and_load(&source, sr) {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("{e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                continue;
                            }

                            let final_path = format!("effects/{effect_name}.lisp");
                            if let Err(e) = std::fs::write(&final_path, &source) {
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

                            // Compilation validated — close editor
                            editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                            editor.remove_buffer_by_name(&buf_name);
                            let slot_idx = editor_effect_slot.take();
                            editor_buffer_name = None;
                            editor_mode = None;
                            editor_effect_name = None;

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

                            // Recompile at the stored slot
                            if let Some(slot_idx) = slot_idx {
                                if let Some(bus_idx) = editor_effect_bus.take() {
                                    match app.load_bus_effect_to_slot_sync(
                                        bus_idx,
                                        slot_idx,
                                        &effect_name,
                                    ) {
                                        Ok(()) => {
                                            *bus_state.lock().unwrap() = app.buses.clone();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Updated bus effect '{effect_name}'"
                                            )));
                                        }
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to reload bus effect: {error}"
                                            )));
                                        }
                                    }
                                } else {
                                    app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                    app.start_effect_compile(&effect_name, slot_idx);
                                    editor.runtime_mut().set_reactive(
                                        "SEQ",
                                        "compiling",
                                        Value::Bool(true),
                                    );
                                }
                            } else {
                                editor_effect_bus = None;
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Saved effect '{effect_name}'"
                                )));
                            }
                        }
                    }

                    "cancel-editor" => {
                        if let Some(buf_name) = editor_buffer_name.take() {
                            editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                            editor.remove_buffer_by_name(&buf_name);
                        }

                        // Clean up temp files for new-* modes
                        if let Some(ref mode) = editor_mode {
                            if mode == "new-instrument" {
                                let _ =
                                    std::fs::remove_file("instruments/.untitled-instrument.lisp");
                            } else if mode == "new-effect" {
                                let _ = std::fs::remove_file("effects/.untitled-effect.lisp");
                            }
                        }
                        editor_mode = None;
                        editor_effect_name = None;
                        editor_effect_slot = None;
                        editor_effect_bus = None;

                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
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
        }

        if app.has_pending_project_load() {
            let was_pending = true;
            match app.advance_pending_project_load() {
                Ok(()) => {
                    if was_pending && !app.has_pending_project_load() {
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
                        current_track.store(0, Ordering::Relaxed);
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
                        let snap_ver = state.scheduler_snapshot_version();
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
                        last_meter_poll_at = Instant::now();
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
                        rt.set_reactive("SEQ", "cpu-load-pct", Value::Number(cpu_load_pct as f64));
                        rt.set_reactive("SEQ", "master-peak-l", Value::Number(cached_peak_l_level));
                        rt.set_reactive("SEQ", "master-peak-r", Value::Number(cached_peak_r_level));
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        rt.set_reactive(
                            "SEQ",
                            "num-tracks",
                            Value::Number(track_names.len() as f64),
                        );
                        rt.set_reactive("SEQ", "current-track", Value::Number(ct as f64));
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
                            sync_track_mixer_empty_state(rt);
                            rt.set_reactive("SEQ", "effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
                            rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
                        } else {
                            sync_all_track_sequencer_state(rt, &state, &app);
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
                            sync_track_params(rt, &app, &state, ct, &selected_steps);
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

                        prev_current_track = ct;
                        prev_playhead = playhead;
                        prev_transport_playhead = transport_playhead;
                        prev_bpm = bpm;
                        prev_playing = playing;
                        prev_pattern_epoch = epoch;
                        prev_snapshot_version = snap_ver;
                        prev_cpu_load_bits = cached_cpu_load_bits;
                        prev_peak_l_level = cached_peak_l_level;
                        prev_peak_r_level = cached_peak_r_level;
                        prev_track_peak_levels = cached_track_peak_levels.clone();
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
        ui_loop_stats.note_host_commands(host_commands_started.elapsed());

        poll_pending_compile_status(
            &mut app,
            &mut editor,
            &state,
            &current_track,
            &selected_steps,
            &fx_epoch,
        );

        // 2. Sync reactive state AFTER events
        let ct = current_track.load(Ordering::Relaxed);
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
            let snap_ver = state.scheduler_snapshot_version();
            let metal_visible = editor_has_visible_buffer(&editor, "*metal*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            let sequencer_visible = editor_has_visible_buffer(&editor, "*sequencer*");
            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let master_meter_visible = transport_visible || mixer_visible;
            let current_track_playhead_visible = editor_has_visible_buffer(&editor, "*metal*")
                || editor_has_visible_buffer(&editor, "*piano-roll*");
            if last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL {
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
                last_meter_poll_at = Instant::now();
            }

            let mut needs_reactive_cycle = false;
            // Track switch — rebuild everything
            if ct != prev_current_track && !app.tracks.is_empty() {
                editor.reset_widget_scroll_for_buffer_named("*metal*");
                editor.reset_widget_scroll_for_buffer_named("*fx*");
                let _ = editor.runtime_mut().eval_str("(set! selected-bus -1)");
                let rt = editor.runtime_mut();
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                rt.set_reactive("SEQ", "current-track", Value::Number(ct as f64));
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
                sync_track_params(rt, &app, &state, ct, &selected_steps);
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
                prev_snapshot_version = snap_ver;
                needs_reactive_cycle = true;
            }

            if playing != prev_playing {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "playing", Value::Bool(playing));
                prev_playing = playing;
                needs_reactive_cycle = true;
            }
            if bpm != prev_bpm {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                prev_bpm = bpm;
                needs_reactive_cycle = true;
            }
            if transport_visible && cpu_load_bits != prev_cpu_load_bits {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "cpu-load-pct",
                    Value::Number(f32::from_bits(cpu_load_bits) as f64),
                );
                prev_cpu_load_bits = cpu_load_bits;
                needs_reactive_cycle = true;
            }
            if !transport_visible && cpu_load_bits != prev_cpu_load_bits {
                prev_cpu_load_bits = cpu_load_bits;
            }
            if master_meter_visible && cached_peak_l_level != prev_peak_l_level {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "master-peak-l",
                    Value::Number(cached_peak_l_level),
                );
                prev_peak_l_level = cached_peak_l_level;
                needs_reactive_cycle = true;
            }
            if !master_meter_visible && cached_peak_l_level != prev_peak_l_level {
                prev_peak_l_level = cached_peak_l_level;
            }
            if master_meter_visible && cached_peak_r_level != prev_peak_r_level {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "master-peak-r",
                    Value::Number(cached_peak_r_level),
                );
                prev_peak_r_level = cached_peak_r_level;
                needs_reactive_cycle = true;
            }
            if !master_meter_visible && cached_peak_r_level != prev_peak_r_level {
                prev_peak_r_level = cached_peak_r_level;
            }
            if cached_track_peak_levels != prev_track_peak_levels {
                if mixer_visible {
                    sync_track_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_track_peak_levels,
                        &cached_track_peak_levels,
                    );
                    needs_reactive_cycle = true;
                }
                prev_track_peak_levels = cached_track_peak_levels.clone();
            }
            if cached_bus_peak_levels != prev_bus_peak_levels {
                if mixer_visible {
                    sync_bus_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_bus_peak_levels,
                        &cached_bus_peak_levels,
                    );
                    needs_reactive_cycle = true;
                }
                prev_bus_peak_levels = cached_bus_peak_levels.clone();
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
                if sync_track_playhead_field_delta(
                    editor.runtime_mut(),
                    &state,
                    &app,
                    &mut prev_track_playheads,
                ) {
                    needs_reactive_cycle = true;
                }
            } else {
                prev_track_playheads = track_playheads_snapshot(&state, &app);
            }
            if current_track_playhead_visible
                && (!prev_current_track_playhead_visible || playhead != prev_playhead)
                && !app.tracks.is_empty()
            {
                if prev_current_track_playhead_visible {
                    sync_playhead_field_delta(
                        editor.runtime_mut(),
                        prev_playhead as usize,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                } else {
                    sync_playhead_fields(
                        editor.runtime_mut(),
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                }
                prev_playhead = playhead;
                needs_reactive_cycle = true;
            }
            if !current_track_playhead_visible && prev_playhead != playhead {
                prev_playhead = playhead;
            }
            prev_current_track_playhead_visible = current_track_playhead_visible;
            if (epoch != prev_pattern_epoch || snap_ver != prev_snapshot_version)
                && !app.tracks.is_empty()
            {
                let rt = editor.runtime_mut();
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                if current_track_playhead_visible {
                    sync_playhead_fields(
                        rt,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                }
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_all_track_sequencer_state(rt, &state, &app);
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_step_param_lists(rt, &state, ct);
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                if mixer_visible {
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                }
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params(rt, &app, &state, ct, &selected_steps);
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                prev_pattern_epoch = epoch;
                prev_snapshot_version = snap_ver;
                prev_track_button_states = track_button_state_snapshot(&state);
                needs_reactive_cycle = true;
            }
            let mut refresh_visible_sequencer_after_cycle = false;
            let mut refresh_visible_mixer_after_cycle = false;
            let ui_ep = ui_epoch.load(Ordering::Relaxed);
            if ui_ep != prev_ui_epoch {
                pull_shared_bus_state(&mut app, &bus_state);
                let track_button_states = track_button_state_snapshot(&state);
                let track_buttons_changed = track_button_states != prev_track_button_states;
                let rt = editor.runtime_mut();
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
                    sync_track_name_state(rt, &mut track_names, &app);
                    sync_track_mixer_state(rt, &app, &state);
                    sync_bus_mixer_state(rt, &app);
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                    *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                    sync_track_params(rt, &app, &state, ct, &selected_steps);
                    rt.set_reactive(
                        "SEQ",
                        "selected-steps",
                        build_selection_value(&selected_steps),
                    );
                    sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                    if sequencer_visible {
                        sync_all_track_sequencer_state(rt, &state, &app);
                    }
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                    );
                }
                // Sync recording state
                let rec_on = recording.load(Ordering::Relaxed);
                rt.set_reactive("SEQ", "recording", Value::Bool(rec_on));
                let armed = record_armed.lock().unwrap();
                let record_armed_changed = armed.len() != app.graph.record_armed.len()
                    || armed
                        .iter()
                        .enumerate()
                        .any(|(i, armed)| app.graph.record_armed.get(i) != Some(armed));
                rt.set_reactive("SEQ", "record-armed", build_record_armed_value(&armed));
                // Sync to app for TUI recording logic
                app.ui.recording = rec_on;
                for (i, a) in armed.iter().enumerate() {
                    if i < app.graph.record_armed.len() {
                        app.graph.record_armed[i] = *a;
                    }
                }
                refresh_visible_sequencer_after_cycle = sequencer_visible;
                refresh_visible_mixer_after_cycle =
                    mixer_visible && (record_armed_changed || track_buttons_changed);
                prev_track_button_states = track_button_states;
                prev_ui_epoch = ui_ep;
                needs_reactive_cycle = true;
            }
            let fx_ep = fx_epoch.load(Ordering::Relaxed);
            if fx_ep != prev_fx_epoch {
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
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "transport-playhead",
                    Value::Number(transport_playhead as f64),
                );
                prev_transport_playhead = transport_playhead;
                needs_reactive_cycle = true;
            }
            if !transport_visible && transport_playhead != prev_transport_playhead {
                prev_transport_playhead = transport_playhead;
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

            if needs_reactive_cycle {
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                if refresh_visible_sequencer_after_cycle {
                    editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                }
                if refresh_visible_mixer_after_cycle {
                    editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                }
                editor.mark_needs_redraw();
            }
        }
        ui_loop_stats.note_sync(reactive_sync_started.elapsed());

        // Keep selection animation live only during playback; when paused, edits/events
        // still request redraws explicitly, but idle should stay cheap.
        if playing_now && !selected_steps.lock().unwrap().is_empty() {
            editor.mark_needs_redraw();
        }

        // Render
        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
            let frame_build_started = Instant::now();
            let tiled_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            let frame_build_elapsed = frame_build_started.elapsed();
            let render_started = Instant::now();
            backend
                .render_tiled(&tiled_frame)
                .map_err(|_| "render failed")?;
            let render_elapsed = render_started.elapsed();
            ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed);
            editor.clear_needs_redraw();
            last_render_at = Instant::now();
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
