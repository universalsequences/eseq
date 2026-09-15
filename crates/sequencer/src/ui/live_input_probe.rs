//! Manual measurement against a real saved project and its production UI.
//! The channel receiver is the software dispatch boundary. This deliberately
//! excludes OS delivery, the router thread, audio block wait and DAC latency.

use crate::*;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub(super) fn run(
    editor: &mut Editor,
    app: &mut app::App,
    shared: &SharedHandles,
    receiver: std::sync::mpsc::Receiver<sequencer::sequencer::LiveInputEvent>,
    cols: u16,
    rows: u16,
) {
    let rack = app
        .groups
        .iter()
        .find(|group| group.rack.is_some())
        .expect("drum rack");
    *shared.armed_rack.lock().unwrap() = Some(rack.id);
    let rack_id = rack.id;
    let keys: Vec<_> = "awsedftgyhujkolp;"
        .chars()
        .filter_map(|key| {
            let note = note_from_key(key)?;
            rack.rack_pad_track(note).map(|track| (key, track))
        })
        .take(2)
        .collect();
    assert_eq!(
        keys.len(),
        2,
        "fixture must have two pads reachable from octave zero"
    );
    assert!(editor.switch_active_tile_to_buffer_named("*sequencer*"));
    editor.blur_all_widget_focus();
    shared.recording.store(true, Ordering::Relaxed);
    app.state.start_playback();
    editor
        .runtime_mut()
        .set_reactive("SEQ", "playing", Value::Bool(true));
    let mut sessions = EditSessionState::default();
    let mut frame = FrameDiffState::default();
    let mut gesture = GestureState::default();
    let mut track_names = app.tracks.clone();
    let mut stats = UiLoopStats::new();
    let mut meters = MeterCache {
        cached_peak_l_level: 0.0,
        cached_peak_r_level: 0.0,
        cached_track_peak_levels: vec![0.0; app.tracks.len()],
        cached_rack_slot_peak_levels: Vec::new(),
        cached_bus_peak_levels: vec![0.0; app.buses.len()],
        cached_modulator_phases: Vec::new(),
        cached_modulator_levels: Vec::new(),
        cached_mod_port_levels: Default::default(),
        cached_mod_display_values: Default::default(),
        watched_display_modulators: Default::default(),
        mod_display_poll_fx_epoch: usize::MAX,
        mod_display_poll_track: None,
        cached_cpu_load_bits: 0,
        last_meter_poll_at: Instant::now(),
        last_cpu_ui_poll_at: Instant::now(),
        last_neural_visualization_poll_at: Instant::now(),
        visualization_liveness: VisualizationLiveness::default(),
        last_voice_count_log_at: Instant::now(),
    };
    let mut frame_number = 0u32;
    let mut finish_frame = |editor: &mut Editor, app: &mut app::App| {
        // Replay the real reactive tick and frame construction that the old
        // event loop interleaved between queued notes. No artificial wait.
        for track in 0..app.tracks.len() {
            let steps = shared.state.pattern.track_params[track]
                .get_num_steps()
                .max(1);
            shared.state.transport.track_playheads[track]
                .store(frame_number % steps as u32, Ordering::Relaxed);
        }
        frame_number += 1;
        pull_named_scratch_buffer_into_project(editor, app);
        sync_reactive_tick(
            app,
            editor,
            &mut LoopCtx {
                sessions: &mut sessions,
                meters: &mut meters,
                frame: &mut frame,
                gesture: &mut gesture,
                track_names: &mut track_names,
                shared,
            },
            &TickInputs {
                cols: cols as usize,
                rows: rows as usize,
                viewport_size: (cols as usize, rows as usize),
                stub_animation_active: false,
                frame_interval: Duration::from_secs_f64(1.0 / 30.0),
                sdf_animation_active: false,
                playing_now: true,
            },
            &mut stats,
        );
        std::hint::black_box(eseqlisp::frame::build_tiled_render_frame_borderless(
            editor,
            cols as usize,
            rows as usize,
        ));
    };
    finish_frame(editor, app);
    for interleave_frame in [true, false] {
        let mut dispatch_us = Vec::new();
        let mut release_us = Vec::new();
        let mut pair_us = Vec::new();
        for iteration in 0..30 {
            let pair_start = Instant::now();
            let mut batch = live_input_batch::LiveInputBatch::new();
            for (index, &(key, track)) in keys.iter().enumerate() {
                let event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE);
                let start = Instant::now();
                batch.begin_event(start);
                let outcome = dispatch_live_keyboard_event(editor, app, shared, &event);
                assert!(outcome.triggered_note());
                let elapsed = start.elapsed().as_secs_f64() * 1e6;
                let sequencer::sequencer::LiveInputEvent::Note(note) = receiver.try_recv().unwrap()
                else {
                    panic!("expected live note");
                };
                assert_eq!(note.track, track);
                assert!(!note.note_off);
                if iteration >= 5 {
                    dispatch_us.push(elapsed);
                }
                if index == 0 {
                    let drain = batch.should_drain(
                        outcome.consumed(),
                        editor.has_pending_host_commands(),
                        Instant::now(),
                    );
                    if interleave_frame || !drain {
                        finish_frame(editor, app);
                    }
                }
            }
            if iteration >= 5 {
                pair_us.push(pair_start.elapsed().as_secs_f64() * 1e6);
            }
            // Release uses the real recording path, outside the note-on interval.
            for &(key, _) in &keys {
                let mut event = KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE);
                event.kind = KeyEventKind::Release;
                let start = Instant::now();
                assert!(dispatch_live_keyboard_event(editor, app, shared, &event).recorded());
                if iteration >= 5 {
                    release_us.push(start.elapsed().as_secs_f64() * 1e6);
                }
                let sequencer::sequencer::LiveInputEvent::Note(note) = receiver.try_recv().unwrap()
                else {
                    panic!("expected note off");
                };
                assert!(note.note_off);
            }
            finish_frame(editor, app);
        }
        let report = |name: &str, mut samples: Vec<f64>| {
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "live-input {name}: median={:.3}us p95={:.3}us max={:.3}us n={}",
                samples[samples.len() / 2],
                samples[(samples.len() - 1) * 95 / 100],
                samples[samples.len() - 1],
                samples.len()
            );
        };
        eprintln!(
            "live-input rack={rack_id} keys={keys:?} tracks={} profile={}",
            app.tracks.len(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        report("dispatch", dispatch_us);
        report(
            if interleave_frame {
                "queued-pair-with-interleaved-frame"
            } else {
                "queued-pair-batched"
            },
            pair_us,
        );
        report("recorded-release", release_us);
    }
    app.state.stop_playback();
}
