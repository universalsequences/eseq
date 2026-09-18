//! Deterministic saved-project UI replay. Uses the real sync/frame/Metal
//! paths with synthetic playheads and process histories. The existing headless
//! audio pump services graph commands; this is not an audio deadline test or
//! a measurement of OS input/display latency. Never writes the saved project.
use crate::*;

pub(super) fn run(editor: &mut Editor, app: &mut app::App, shared: &SharedHandles) {
    let out = std::path::PathBuf::from(std::env::var("ESEQ_UI_REPLAY_OUT")
        .expect("set ESEQ_UI_REPLAY_OUT to the report JSON path"));
    if let Some(parent) = out.parent() { std::fs::create_dir_all(parent).unwrap(); }
    // The fixture loaded its project after creating SharedHandles. Mirror the
    // live project-load publication before the first tick can pull stale buses
    // back into App and discard the project's group buses.
    *shared.bus_state.lock().unwrap() = app.buses.clone();
    *shared.track_groups.lock().unwrap() = app.groups.clone();
    let extra_tracks: usize = std::env::var("ESEQ_UI_REPLAY_EMPTY_TRACKS")
        .ok().map(|value| value.parse().expect("empty track count")).unwrap_or(0);
    for _ in 0..extra_tracks {
        app.graph_controller().add_blank_sampler_track().expect("add empty sampler");
    }
    let (width, height) = (2000, 1200);
    let mut backend = create_offscreen_capture_backend(editor, width, height, 1.0).unwrap();
    let (cell_w, cell_h) = backend.cell_dimensions();
    let (cols, rows) = ((width as f32 / cell_w) as usize, (height as f32 / cell_h) as usize);
    let target = backend.create_tiled_capture_target(width, height).unwrap_or_else(|_| panic!("create replay target"));
    editor.set_layout_viewport(cols as u16, rows as u16);
    editor.update_tile_rects(cols as u16, rows as u16);
    shared.current_track.store(0, Ordering::Relaxed);
    let solo_replay = std::env::var("ESEQ_UI_REPLAY_MODE").as_deref() == Ok("solo");
    if solo_replay {
        shared.state.stop_playback();
    } else {
        shared.state.start_playback();
    }
    editor.runtime_mut().set_reactive("SEQ", "playing", Value::Bool(!solo_replay));
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

    let mut reports = Vec::new();
    if solo_replay {
        let bus_count = app.buses.len();
        // Exercise the real native -> targeted invalidation -> reactive tick ->
        // frame path. Selection is held fixed to isolate mute/solo publication.
        for _ in 0..4 {
            editor.sync_reactive_bindings_for_visible_layouts();
            sync_reactive_tick(app, editor, &mut LoopCtx {
                sessions: &mut sessions, meters: &mut meters, frame: &mut frame,
                gesture: &mut gesture, track_names: &mut track_names, shared,
            }, &TickInputs { cols, rows, playing_now: false }, &mut stats);
            let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
            backend.render_tiled_capture(&tiled, &target).unwrap_or_else(|_| panic!("render solo warmup"));
        }
        assert_eq!(app.buses.len(), bus_count, "warmup must retain every project bus");
        for (native, count) in [
            ("seq-toggle-track-solo", app.tracks.len()),
            ("seq-toggle-track-mute", app.tracks.len()),
            ("seq-toggle-bus-solo", app.buses.len()),
            ("seq-toggle-bus-mute", app.buses.len()),
        ] {
            for index in 0..count {
                for toggle in 0..2 {
                    let enabled = |app: &app::App| match native {
                        "seq-toggle-track-solo" => app.state.pattern.track_params[index].is_solo(),
                        "seq-toggle-track-mute" => app.state.pattern.track_params[index].is_muted(),
                        "seq-toggle-bus-solo" => app.buses[index].solo,
                        "seq-toggle-bus-mute" => app.buses[index].mute,
                        _ => unreachable!(),
                    };
                    let was_enabled = enabled(app);
                    let before = editor.runtime().ui_work_counters();
                    let started = Instant::now();
                    editor.runtime_mut().eval_str(&format!("({native} {index})")).unwrap();
                    let commands = editor.drain_host_commands();
                    assert!(!commands.is_empty(), "{native} must enqueue its history command");
                    for command in commands {
                        let eseqlisp::HostCommand::Custom { name, payload } = command else {
                            panic!("unexpected solo probe host command");
                        };
                        dispatch_custom_host_command(&name, payload, app, editor, &mut LoopCtx {
                            sessions: &mut sessions, meters: &mut meters, frame: &mut frame,
                            gesture: &mut gesture, track_names: &mut track_names, shared,
                        });
                    }
                    assert_ne!(enabled(app), was_enabled, "{native} {index} must change actual state");
                    sync_reactive_tick(app, editor, &mut LoopCtx {
                        sessions: &mut sessions, meters: &mut meters, frame: &mut frame,
                        gesture: &mut gesture, track_names: &mut track_names, shared,
                    }, &TickInputs { cols, rows, playing_now: false }, &mut stats);
                    let sync_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let build_started = Instant::now();
                    let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
                    let build_ms = build_started.elapsed().as_secs_f64() * 1000.0;
                    let render = backend.render_tiled_capture(&tiled, &target).unwrap_or_else(|_| panic!("render solo frame"));
                    let after = editor.runtime().ui_work_counters();
                    reports.push(serde_json::json!({"native": native, "index": index, "toggle": toggle,
                        "enabled": enabled(app), "sync_ms": sync_ms, "frame_build_ms": build_ms, "render": render,
                        "full_reruns": after.full_buffer_reruns - before.full_buffer_reruns,
                        "subtree_reruns": after.subtree_reruns - before.subtree_reruns,
                        "relayout_full": after.relayout_full - before.relayout_full,
                        "relayout_subtree": after.relayout_subtree - before.relayout_subtree}));
                    assert_eq!(after, before, "{native} {index} toggle {toggle} rebuilt UI: {:?}",
                        editor.runtime().last_ui_invalidation_trace());
                    if native == "seq-toggle-track-solo" && index == 0 && toggle == 0 {
                        target.save_png(&out.with_extension("png")).unwrap();
                    }
                }
            }
        }
        std::fs::write(&out, serde_json::to_vec_pretty(&serde_json::json!({
            "scope": "saved-project mute/solo native dispatch, reactive sync, frame and Metal capture; fixed selection; no OS input or display scanout",
            "project": std::env::var("ESEQ_UI_REPLAY_PROJECT").unwrap(),
            "tracks": app.tracks.len(), "buses": app.buses.len(), "samples": reports,
        })).unwrap()).unwrap();
        eprintln!("Solo replay report: {}", out.display());
        return;
    }
    for phase in ["panels", "live-panels", "scroll", "scratch"] {
        if phase == "scratch" {
            let buffer = editor.buffers.iter().find(|buffer| buffer.name == "*scratch*").expect("scratch").id;
            editor.set_active_buffer(buffer);
            editor.runtime_mut().eval_str("(delete-other-windows)").unwrap();
            editor.refresh_runtime_side_effects();
        } else {
            assert!(editor.switch_active_tile_to_buffer_named(if phase == "scroll" { "*sequencer*" } else { "*fx*" }));
        }
        editor.mark_needs_redraw();
        let hidden_fields = ["track-events", "track-event-current-beat", "track-active-notes", "track-process-scopes", "transport-playhead"];
        let hidden_snapshot = (phase == "scratch").then(|| hidden_fields.map(|field|
            editor.runtime().reactive_field_value("SEQ", field).map(Value::deep_clone)));
        let compressor_keys = editor.visible_widget_layouts().iter().flat_map(|layout|
            eseqlisp::widget_render::compressor_display::collect_compressor_meter_requests(layout)
                .into_iter().map(|request| request.data_key)).collect::<std::collections::HashSet<_>>();
        let spectra = if std::env::var_os("ESEQ_UI_REPLAY_SPECTRA").is_some() {
            editor.visible_widget_layouts().iter().flat_map(|layout|
                eseqlisp::widget_render::spectrogram::collect_spectrogram_requests(layout)).collect::<Vec<_>>()
        } else { Vec::new() };
        if phase == "live-panels" { assert!(!compressor_keys.is_empty(), "fixture needs a visible compressor"); }
        eprintln!("replay {phase}: {} mounted spectra, {} compressors", spectra.len(), compressor_keys.len());
        for index in 0..210 {
            for track in 0..app.tracks.len() {
                let steps = shared.state.pattern.track_params[track].get_num_steps().max(1);
                shared.state.transport.track_playheads[track].store((index / 6) % steps as u32, Ordering::Relaxed);
            }
            // Histories use real process owners and state-cell names, so the
            // normal projection and authored reactive readers are exercised.
            let published = shared.state.published_process_authoring();
            let mut histories = std::collections::HashMap::new();
            for track in 0..app.tracks.len() {
                if let Some(chain) = shared.state.composed_track_process_chain(track) {
                    for slot in &chain.slots {
                        let id = sequencer::process::track_process_slot_runtime_id(slot, track).0;
                        let name = published.defs.iter().find(|def| def.name == slot.class_name)
                            .and_then(|def| def.state.first()).map(|cell| cell.name.clone()).unwrap_or("value".into());
                        histories.insert(id, std::collections::HashMap::from([(name,
                            (0..64).map(|sample| ((index + sample) as f32 * 0.1).sin()).collect())]));
                    }
                }
            }
            shared.state.publish_process_scope_values(histories);
            // Match the live analyzer's publication boundary without depending
            // on audio clock scheduling. Keys come from the mounted widgets;
            // history length and stride match the real compressor ring.
            if matches!(phase, "live-panels" | "scroll") {
                for key in &compressor_keys {
                    eseqlisp::live_audio::publish_compressor_meter_frame(key.clone(), eseqlisp::live_audio::CompressorMeterFrame {
                        revision: index as u64, gr_db: -3.0, out_db: -12.0, sample_rate: 48_000.0,
                        stride: sequencer::effects::compressor::METER_STRIDE,
                        history: std::sync::Arc::new((0..sequencer::effects::compressor::METER_RING_LEN)
                            .map(|sample| [-12.0 + ((sample + index as usize) as f32 * 0.1).sin() * 6.0, -3.0]).collect()),
                    });
                }
                for request in &spectra {
                    let bins = request.fft_size / 2 + 1;
                    let smoothed: Vec<_> = (0..bins).map(|bin|
                        (0.45 + 0.3 * ((bin + index as usize) as f32 * 0.07).sin()).max(0.0)).collect();
                    eseqlisp::live_audio::publish_spectrogram_frame(&request.data_key, eseqlisp::live_audio::SpectrogramFrame {
                        revision: index as u64, bins: bins as u32, time_slices: request.time_slices as u32,
                        write_head: index % request.time_slices as u32, sample_rate: 48_000.0,
                        waterfall: std::sync::Arc::new(smoothed.repeat(request.time_slices)),
                        smoothed: std::sync::Arc::new(smoothed),
                    });
                }
                if !compressor_keys.is_empty() || !spectra.is_empty() { editor.mark_needs_redraw(); }
            }
            // Exercise playback publishers even when the replay runs faster
            // than their wall-clock cadence. The previous probe left these
            // mostly idle, hiding work paid by actual scratch-only playback.
            meters.last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL;
            meters.last_neural_visualization_poll_at = Instant::now() - NEURAL_VISUALIZATION_POLL_INTERVAL;
            shared.state.transport.playhead.store(index, Ordering::Relaxed);
            shared.state.append_track_output_events([sequencer::sequencer::TrackOutputEvent {
                track: 0, sample_time: index as u64 * 512, beat: index as f64 / 6.0,
                transpose: 0.0, velocity: 1.0,
            }]);
            let started = Instant::now();
            if phase == "scroll" { editor.apply_smooth_widget_scroll(0.0, if index % 60 < 30 { -0.5 } else { 0.5 }); }
            editor.sync_reactive_bindings_for_visible_layouts();
            sync_reactive_tick(app, editor, &mut LoopCtx {
                sessions: &mut sessions, meters: &mut meters, frame: &mut frame,
                gesture: &mut gesture, track_names: &mut track_names, shared,
            }, &TickInputs {
                cols, rows,
                playing_now: true,
            }, &mut stats);
            let sync_ms = started.elapsed().as_secs_f64() * 1000.0;
            let redraw = editor.needs_redraw();
            if index >= 30 && phase == "scratch" {
                assert!(!redraw, "hidden playback displays must not request scratch frames");
                assert!(meters.watched_display_modulators.is_empty());
                assert!(frame.watched_sampler_voice_ids.is_empty());
                let current = hidden_fields.map(|field|
                    editor.runtime().reactive_field_value("SEQ", field).map(Value::deep_clone));
                assert_eq!(hidden_snapshot.as_ref(), Some(&current), "hidden display fields must not be rebuilt");
            }
            let mut build_ms = 0.0;
            let mut render = None;
            if redraw {
                let build_started = Instant::now();
                let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
                build_ms = build_started.elapsed().as_secs_f64() * 1000.0;
                render = Some(backend.render_tiled_capture(&tiled, &target).unwrap_or_else(|_| panic!("render replay frame")));
                editor.clear_needs_redraw();
            }
            if index >= 30 {
                reports.push(serde_json::json!({"phase": phase, "redraw": redraw,
                    "sync_ms": sync_ms, "frame_build_ms": build_ms, "render": render,
                    "cpu_ms": sync_ms + build_ms + render.map_or(0.0, |render| render.cpu_ms)}));
            }
        }
        target.save_png(&out.with_file_name(format!("{}-{phase}.png", out.file_stem().unwrap().to_string_lossy()))).unwrap();
    }
    // Restore the real panel layout and verify its first sync samples meters
    // even though their ordinary wall-clock interval has not elapsed.
    editor.runtime_mut().eval_str("(eseq.seq-layout/apply-fx-layout)").unwrap();
    editor.refresh_runtime_side_effects();
    editor.update_tile_rects(cols as u16, rows as u16);
    editor.sync_reactive_bindings_for_visible_layouts();
    let scope_version_before_reopen = frame.prev_process_scope_values_version;
    meters.cached_peak_l_level = -1.0;
    meters.cached_track_peak_levels.clear();
    meters.last_meter_poll_at = Instant::now();
    sync_reactive_tick(app, editor, &mut LoopCtx {
        sessions: &mut sessions, meters: &mut meters, frame: &mut frame,
        gesture: &mut gesture, track_names: &mut track_names, shared,
    }, &TickInputs {
        cols, rows,
        playing_now: true,
    }, &mut stats);
    assert!(meters.cached_peak_l_level >= 0.0, "reopened master meter samples immediately");
    assert_eq!(meters.cached_track_peak_levels.len(), app.tracks.len());
    if editor.runtime().has_live_reactive_consumers("SEQ", "track-process-scopes") {
        assert_eq!(frame.prev_process_scope_values_version, shared.state.process_scope_values_version());
    } else {
        // Open track groups do not imply expanded lane editors. A restored
        // layout without scope consumers must continue leaving histories alone.
        assert_eq!(frame.prev_process_scope_values_version, scope_version_before_reopen);
    }
    let tiled = eseqlisp::frame::build_tiled_render_frame_borderless(editor, cols, rows);
    backend.render_tiled_capture(&tiled, &target).unwrap_or_else(|_| panic!("render reopened panels"));
    target.save_png(&out.with_file_name(format!("{}-reopened.png", out.file_stem().unwrap().to_string_lossy()))).unwrap();
    let report = serde_json::json!({
        "scope": "saved-project UI replay with synthetic playheads/process histories/compressor rings; no scheduler, OS input or display scanout",
        "project": std::env::var("ESEQ_UI_REPLAY_PROJECT").unwrap(), "tracks": app.tracks.len(),
        "groups_open": app.groups.iter().all(|group| !group.collapsed), "width": width, "height": height,
        "samples": reports,
    });
    std::fs::write(&out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!("UI replay report: {}", out.display());
}
