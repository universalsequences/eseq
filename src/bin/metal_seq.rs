use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::Event;

use eseqlisp::backend::Backend;
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::vm::Value;
use eseqlisp::{Editor, EditorConfig, Runtime};

use sequencer::engine;
use sequencer::sequencer::{SequencerState, StepParam};
use sequencer::ui;

const DEFAULT_SAMPLES: &[&str] = &[
    "samples/producers/Boom-Bap/Boom-Bap Kick 51.wav",
    "samples/producers/madlib/Snare SwaggedOut 3.wav",
    "samples/producers/donda/PABLO HAT.wav",
];
const NUM_STEPS: usize = 16;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Init audio engine
    let eng = engine::init_engine()?;
    let lg_ptr = eng.lg_ptr;
    let state = eng.state.clone();
    let stream = eng._stream;

    // 2. Create App and add default tracks
    let mut app = ui::App::new(
        eng.state.clone(),
        eng.lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder,
        eng.keyboard_tx,
    );

    let mut track_names: Vec<String> = Vec::new();
    for sample_path in DEFAULT_SAMPLES {
        let path = Path::new(sample_path);
        if path.exists() {
            let idx = app.graph_controller().add_track(path)?;
            let name = app.tracks[idx].clone();
            eprintln!("metal_seq: track {idx} = {name} ({sample_path})");
            track_names.push(name);
        } else {
            eprintln!("metal_seq: skipping missing sample: {sample_path}");
        }
    }

    if track_names.is_empty() {
        return Err("No samples found".into());
    }

    // Collect pan node IDs for volume control (need to push params to audiograph)
    let track_pan_ids: Vec<i32> = app.graph.track_node_ids.iter().map(|n| n.pan_id).collect();
    let lg_raw = lg_ptr.0;

    // Shared current track index
    let current_track = Arc::new(AtomicUsize::new(0));
    // UI-only counter for changes that shouldn't affect pattern_epoch (e.g. volume)
    let ui_epoch = Arc::new(AtomicUsize::new(0));

    // 3. Set up eseqlisp runtime with sequencer natives
    let mut runtime = Runtime::new();

    let ct = current_track.clone();
    let track_count = track_names.len();

    // Register SEQ reactive namespace
    runtime.register_reactive(
        "SEQ",
        vec![
            ("playing", Value::Bool(false)),
            ("bpm", Value::Number(120.0)),
            ("num-steps", Value::Number(NUM_STEPS as f64)),
            ("num-tracks", Value::Number(track_count as f64)),
            ("current-track", Value::Number(0.0)),
            ("playhead", Value::Number(0.0)),
            ("track-names", build_track_names(&track_names)),
            ("steps", build_steps_value(&state, 0)),
            ("velocities", build_param_list(&state, 0, StepParam::Velocity)),
            ("durations", build_param_list(&state, 0, StepParam::Duration)),
            ("transposes", build_param_list(&state, 0, StepParam::Transpose)),
            ("pans", build_param_list(&state, 0, StepParam::Pan)),
            ("track-volumes", build_track_volumes(&state)),
        ],
        false,
    );

    // ── Native functions ──

    // seq-toggle-step — toggle step on current track
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-toggle-step", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-toggle-step: expected step number".into());
        };
        let step = *step as usize;
        if step >= NUM_STEPS {
            return Err(format!("seq-toggle-step: step {step} out of range").into());
        }
        let track = ct.load(Ordering::Relaxed);
        st.toggle_step_and_clear_plocks(track, step);
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    // seq-set-step-param — set param on current track
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-set-step-param", move |args, _ctx| {
        let (Some(Value::Number(step)), Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-step-param: expected (step :param value)".into());
        };
        let step = *step as usize;
        if step >= NUM_STEPS {
            return Err(format!("seq-set-step-param: step {step} out of range").into());
        }
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "speed" => StepParam::Speed,
            other => return Err(format!("seq-set-step-param: unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        st.pattern.step_data[track].set(step, param, val);
        st.publish_scheduler_snapshot();
        Ok(Value::Number(val as f64))
    });

    // seq-set-track — switch current track
    let ct = current_track.clone();
    runtime.register_native("seq-set-track", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-set-track: expected track number".into());
        };
        let track = *track as usize;
        if track >= track_count {
            return Err(format!("seq-set-track: track {track} out of range").into());
        }
        ct.store(track, Ordering::Relaxed);
        Ok(Value::Number(track as f64))
    });

    // seq-set-track-volume — (seq-set-track-volume track-idx volume)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-track-volume", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(vol))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-track-volume: expected (track volume)".into());
        };
        let track = *track as usize;
        let vol = (*vol as f32).clamp(0.0, 1.0);
        st.pattern.track_params[track].set_volume(vol);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        // Push volume to audiograph's stereo panner node
        if let Some(&pan_id) = pan_ids.get(track) {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: pan_id as u64,
                        fvalue: vol,
                    },
                );
            }
        }
        Ok(Value::Number(vol as f64))
    });

    // seq-toggle-play
    let st = state.clone();
    runtime.register_native("seq-toggle-play", move |_args, _ctx| {
        let playing = st.transport.playing.load(Ordering::Relaxed);
        if playing {
            st.transport.playing.store(false, Ordering::Relaxed);
            st.publish_scheduler_snapshot();
        } else {
            st.transport.playing.store(true, Ordering::Relaxed);
            st.transport.playhead.store(0, Ordering::Relaxed);
            st.publish_scheduler_snapshot();
        }
        Ok(Value::Bool(!playing))
    });

    let st = state.clone();
    runtime.register_native("seq-set-bpm", move |args, _ctx| {
        let Some(Value::Number(bpm)) = args.first() else {
            return Err("seq-set-bpm: expected bpm number".into());
        };
        let bpm = (*bpm as u32).clamp(20, 300);
        st.transport.bpm.store(bpm, Ordering::Relaxed);
        st.publish_scheduler_snapshot();
        Ok(Value::Number(bpm as f64))
    });

    // 4. Create editor with Metal backend
    let init_src = std::fs::read_to_string("init.lisp")
        .or_else(|_| std::fs::read_to_string("../eseqlisp/init.lisp"))
        .unwrap_or_default();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
        },
    );

    let _ = editor.open_or_create_file_buffer("metal-seq-grid.lisp");

    let mut backend = MetalBackend::new().map_err(|_| "Metal backend creation failed")?;
    backend.initialize().map_err(|_| "Metal backend init failed")?;

    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
    }

    // 5. Metal event loop
    let frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let mut last_render_at = Instant::now() - frame_interval;
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;

    let mut prev_playing = false;
    let mut prev_bpm: u32 = 0;
    let mut prev_playhead: u32 = u32::MAX;
    let mut prev_pattern_epoch: u64 = 0;
    let mut prev_snapshot_version: u64 = 0;
    let mut prev_current_track: usize = usize::MAX;
    let mut prev_ui_epoch: usize = 0;

    eprintln!("metal_seq: entering event loop");

    loop {
        editor.update_timers();
        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);

        // 1. Poll events FIRST
        let playing_now = state.transport.playing.load(Ordering::Relaxed);
        let timeout = if playing_now {
            frame_interval.saturating_sub(last_render_at.elapsed()).max(Duration::from_millis(8))
        } else if editor.needs_redraw() {
            Duration::from_millis(4)
        } else {
            Duration::from_millis(50)
        };
        match backend.poll_event(timeout) {
            Some(Event::Key(key)) => editor.handle_key(key),
            Some(Event::Mouse(mouse)) => {
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
            Some(Event::Resize(_, _)) => editor.mark_needs_redraw(),
            _ => {}
        }

        // Touchpad gestures
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

        // Flush pending drag at frame boundary
        if last_render_at.elapsed() >= frame_interval {
            if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
        }

        // 2. Sync reactive state AFTER events
        let ct = current_track.load(Ordering::Relaxed);
        {
            let playing = state.transport.playing.load(Ordering::Relaxed);
            let bpm = state.transport.bpm.load(Ordering::Relaxed);
            let playhead = state.transport.track_playheads[ct].load(Ordering::Relaxed);
            let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            let snap_ver = state.scheduler_snapshot_version();

            let mut needs_reactive_cycle = false;

            // Track switch — rebuild everything
            if ct != prev_current_track {
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "current-track", Value::Number(ct as f64));
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                rt.set_reactive("SEQ", "velocities", build_param_list(&state, ct, StepParam::Velocity));
                rt.set_reactive("SEQ", "durations", build_param_list(&state, ct, StepParam::Duration));
                rt.set_reactive("SEQ", "transposes", build_param_list(&state, ct, StepParam::Transpose));
                rt.set_reactive("SEQ", "pans", build_param_list(&state, ct, StepParam::Pan));
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                prev_current_track = ct;
                prev_pattern_epoch = epoch;
                prev_snapshot_version = snap_ver;
                needs_reactive_cycle = true;
            }

            if playing != prev_playing {
                editor.runtime_mut().set_reactive("SEQ", "playing", Value::Bool(playing));
                prev_playing = playing;
                needs_reactive_cycle = true;
            }
            if bpm != prev_bpm {
                editor.runtime_mut().set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                prev_bpm = bpm;
                needs_reactive_cycle = true;
            }
            if epoch != prev_pattern_epoch || snap_ver != prev_snapshot_version {
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                rt.set_reactive("SEQ", "velocities", build_param_list(&state, ct, StepParam::Velocity));
                rt.set_reactive("SEQ", "durations", build_param_list(&state, ct, StepParam::Duration));
                rt.set_reactive("SEQ", "transposes", build_param_list(&state, ct, StepParam::Transpose));
                rt.set_reactive("SEQ", "pans", build_param_list(&state, ct, StepParam::Pan));
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                prev_pattern_epoch = epoch;
                prev_snapshot_version = snap_ver;
                needs_reactive_cycle = true;
            }
            let ui_ep = ui_epoch.load(Ordering::Relaxed);
            if ui_ep != prev_ui_epoch {
                editor.runtime_mut().set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                prev_ui_epoch = ui_ep;
                needs_reactive_cycle = true;
            }
            if playhead != prev_playhead {
                editor.runtime_mut().set_reactive("SEQ", "playhead", Value::Number(playhead as f64));
                prev_playhead = playhead;
                needs_reactive_cycle = true;
            }

            if needs_reactive_cycle {
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
            }
        }

        // Render
        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
            let tiled_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            backend.render_tiled(&tiled_frame).map_err(|_| "render failed")?;
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

/// Build a Lisp Value::List of bools from the step pattern for a given track.
fn build_steps_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..NUM_STEPS)
        .map(|s| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.patterns[track].is_active(s),
            )))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of floats for a given step param on a given track.
fn build_param_list(state: &Arc<SequencerState>, track: usize, param: StepParam) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..NUM_STEPS)
        .map(|s| {
            let val = state.pattern.step_data[track].get(s, param);
            Rc::new(RefCell::new(Value::Number(val as f64)))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of track name strings.
fn build_track_names(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of per-track volumes (0.0–1.0).
fn build_track_volumes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let vol = state.pattern.track_params[t].get_volume();
            Rc::new(RefCell::new(Value::Number(vol as f64)))
        })
        .collect();
    Value::List(items)
}
