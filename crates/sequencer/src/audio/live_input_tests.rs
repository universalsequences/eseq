use super::*;

#[test]
fn live_scene_transpose_reaches_synth_and_preserves_held_note_identity() {
    use crate::effects::gatepitch as gp;
    use crate::sequencer::{LiveInputEvent, LiveNoteSource, SCENE_TRANSPOSE_SLOT};

    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    struct GraphGuard(engine::HeadlessEngine);
    impl Drop for GraphGuard {
        fn drop(&mut self) { unsafe { self.0.destroy(); } }
    }
    let guard = GraphGuard(engine);
    let engine = &guard.0;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = crate::app::App::new(
        engine.state.clone(), engine.lg_ptr, engine.sample_rate,
        engine.buses.clone(), engine.master_recorder.clone(), tx.clone(),
    );
    // Keep the actual host voice graph and GatePitch event ABI, without a
    // compiler or audio device: the synth at the end of the graph is silent.
    let manifest = crate::lisp_host::DGenManifest {
        effect_latency_samples: None, dylib_path: Default::default(), asset_base: None,
        version: 1, process_abi: String::new(), total_memory_slots: 1,
        params: vec![], groups: vec![], envelopes: vec![], inputs: vec![],
        modulators: vec![], mod_outputs: vec![], mod_destinations: vec![],
        n_inputs: 4, n_outputs: 1, tensors: vec![], tensor_init_data: vec![],
        voice_cell_id: None,
    };
    let lib = crate::lisp_host::test_loaded_dgen_lib();
    let engine_id = app.editor.engine_registry.upsert(crate::app::EngineDescriptor {
        name: "live-transpose".into(), source: "live-transpose.lisp".into(),
        manifest: manifest.clone(), lib_index: 0, shared_runtime: true,
    });
    app.editor.instrument_libs.push(crate::lisp_host::test_loaded_dgen_lib());
    app.graph_controller().add_custom_track(
        "live-transpose", engine_id, &manifest, &lib, CustomInstrumentRunMode::Instrument,
    ).unwrap();
    let state = &engine.state;
    state.pattern.track_params[0].set_max_polyphony(1);
    if !state.pattern.track_params[0].is_gate_on() {
        state.pattern.track_params[0].toggle_gate();
    }
    let gatepitch = app.graph.engine_node_ids[engine_id].as_ref().unwrap().gatepitch_ids[0];
    unsafe { assert!(add_node_to_watchlist(engine.lg_ptr.0, gatepitch)); }
    let mut data = new_audio_callback_data(
        engine.lg_ptr.0, state.clone(), 48_000, 2, 512,
        engine.master_recorder.clone(), rx, engine.buses.bus_effect_runtime.clone(),
        Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
    );
    let mut output = vec![0.0; 1024];
    let mut render = |data: &mut AudioCallbackData| {
        // Watched node state is published every four graph blocks.
        for _ in 0..4 { audio_callback(data, &mut output); }
    };
    let assert_voice = |transpose: f32, gate: f32| {
        let mut slots = vec![0.0_f32; gp::GATEPITCH_STATE_SIZE];
        let mut size = 0;
        unsafe { assert!(get_node_state_into(engine.lg_ptr.0, gatepitch,
            slots.as_mut_ptr().cast(), std::mem::size_of_val(slots.as_slice()), &mut size)); }
        let expected_hz = 440.0 * 2f32.powf((transpose - 9.0) / 12.0);
        assert!((slots[gp::PARAM_PITCH as usize] - expected_hz).abs() < 0.001,
            "synth pitch {} must match {expected_hz}", slots[gp::PARAM_PITCH as usize]);
        assert_eq!(slots[gp::PARAM_GATE as usize], gate);
    };
    for (playing, semitones, enabled) in [
        (false, 8.0, true), (true, -5.0, true), (true, 0.0, true), (true, 8.0, false),
    ] {
        state.transport.playing.store(playing, Ordering::Relaxed);
        state.pattern.track_params[0].set_global_transpose(enabled);
        state.write_current_scene_slot(SCENE_TRANSPOSE_SLOT,
            crate::process::ProcessLiteral::Number(semitones)).unwrap();
        let first = KeyboardTrigger {
            generation: 1, source: Some(LiveNoteSource::Key('s')), track: 0,
            transpose: 2.0, velocity: 0.7, note_off: false,
        };
        tx.send(LiveInputEvent::Note(first)).unwrap();
        render(&mut data);
        let first_pitch = 2.0 + if enabled { semitones as f32 } else { 0.0 };
        assert_voice(first_pitch, 1.0);
        let held = data.active_keyboard_notes[0].iter().flatten().next().unwrap();
        assert_eq!(held.source_transpose, 2.0, "release and recording use the input pitch");
        assert_eq!(held.midi_note, Some((60.0 + first_pitch) as u8));

        // A later note sees a live edit; releasing it must restore the older
        // hold at its original sounding pitch and ultimately close its gate.
        state.write_current_scene_slot(SCENE_TRANSPOSE_SLOT,
            crate::process::ProcessLiteral::Number(semitones + 3.0)).unwrap();
        let second = KeyboardTrigger {
            generation: 2, source: Some(LiveNoteSource::Midi { port: 0, channel: 0, note: 67 }),
            transpose: 7.0, ..first
        };
        tx.send(LiveInputEvent::Note(second)).unwrap();
        render(&mut data);
        assert_voice(7.0 + if enabled { semitones as f32 + 3.0 } else { 0.0 }, 1.0);
        tx.send(LiveInputEvent::Note(KeyboardTrigger { note_off: true, ..second })).unwrap();
        render(&mut data);
        assert_voice(first_pitch, 1.0);
        tx.send(LiveInputEvent::Note(KeyboardTrigger { note_off: true, ..first })).unwrap();
        render(&mut data);
        assert_voice(first_pitch, 0.0);
        assert!(data.active_keyboard_notes[0].iter().all(Option::is_none));
        let mut stamps = Vec::new();
        state.drain_live_trigger_stamps(|stamp| stamps.push(stamp.transpose));
        assert_eq!(stamps, if playing { vec![2.0, 7.0] } else { vec![] },
            "recording stamps retain source pitch; resumed holds do not record twice");
    }
}

/// Complements the UI dispatch probe with the real callback and saved DSP.
/// No device, scheduler lookahead, UI, or synthetic delay is involved here.
#[test]
#[ignore = "manual saved-project probe; set ESEQ_LIVE_INPUT_PROJECT"]
fn saved_drum_rack_live_notes_share_a_render_sample() {
    let project = std::path::PathBuf::from(
        std::env::var("ESEQ_LIVE_INPUT_PROJECT")
            .expect("set ESEQ_LIVE_INPUT_PROJECT to an absolute saved-project path"),
    );
    let engine = engine::init_headless_engine(48_000, 2).unwrap();
    struct GraphGuard(*mut LiveGraph);
    impl Drop for GraphGuard {
        fn drop(&mut self) {
            unsafe {
                engine_stop_workers();
                destroy_live_graph(self.0);
            }
        }
    }
    let _guard = GraphGuard(engine.lg_ptr.0);
    let mut app = crate::app::App::new(
        engine.state.clone(),
        engine.lg_ptr,
        engine.sample_rate,
        engine.buses.clone(),
        engine.master_recorder.clone(),
        engine.keyboard_tx.clone(),
    );
    // Project loading posts graph edits. Pump them while the host waits, then
    // join before giving the production callback exclusive graph ownership.
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = running.clone();
    let graph = engine.lg_ptr;
    let pump = std::thread::spawn(move || {
        let mut output = vec![0.0; 512 * 2];
        while flag.load(Ordering::Relaxed) {
            unsafe {
                graph.process_next_block(output.as_mut_ptr(), 512);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    struct PumpGuard {
        running: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for PumpGuard {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }
    let pump = PumpGuard {
        running,
        thread: Some(pump),
    };
    let load_result = (|| {
        app.queue_project_load_from_path("live-input-probe", &project)?;
        while app.has_pending_project_load() {
            app.advance_pending_project_load()?;
        }
        Ok::<_, String>(())
    })();
    drop(pump);
    load_result.unwrap();
    let group = app
        .groups
        .iter()
        .find(|group| group.rack.is_some())
        .unwrap();
    let tracks: Vec<_> = group
        .rack
        .as_ref()
        .unwrap()
        .pads
        .iter()
        .take(2)
        .map(|pad| group.members[pad.member])
        .collect();
    assert_eq!(tracks.len(), 2);
    assert!(
        tracks
            .iter()
            .all(|&track| app.state.pattern.track_params[track]
                .midi_fx_chain()
                .is_empty()),
        "this probe covers immediate live input, not authored MIDI FX"
    );
    app.state.start_playback();
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut data = session::new_audio_callback_data(
        engine.lg_ptr.0,
        engine.state.clone(),
        48_000,
        2,
        512,
        engine.master_recorder.clone(),
        receiver,
        engine.buses.bus_effect_runtime.clone(),
        Arc::new(ScheduledEventQueue::new()),
        Arc::new(AtomicU64::new(0)),
    );
    let mut output = vec![0.0; 512 * 2];
    callback::render_audio_block(
        &mut data,
        &mut output,
        callback::AudioOutputPurpose::Playback,
    );
    let mut durations = Vec::new();
    let mut peak = 0.0_f32;
    for generation in 1..=25 {
        let start = Instant::now();
        for &track in &tracks {
            sender
                .send(crate::sequencer::LiveInputEvent::Note(KeyboardTrigger {
                    generation,
                    source: None,
                    track,
                    transpose: 0.0,
                    velocity: 1.0,
                    note_off: false,
                }))
                .unwrap();
        }
        callback::render_audio_block(
            &mut data,
            &mut output,
            callback::AudioOutputPurpose::Playback,
        );
        durations.push(start.elapsed().as_secs_f64() * 1e6);
        let mut stamps = Vec::new();
        engine
            .state
            .drain_live_trigger_stamps(|stamp| stamps.push(stamp));
        assert_eq!(stamps.len(), 2);
        assert_eq!(
            stamps[0].beat, stamps[1].beat,
            "queued pads must fire at the same render sample"
        );
        for sample in &output {
            assert!(sample.is_finite());
            peak = peak.max(sample.abs());
        }
        for &track in &tracks {
            assert!(
                data.active_keyboard_notes[track]
                    .iter()
                    .any(Option::is_some),
                "pad must own a sounding voice"
            );
            sender
                .send(crate::sequencer::LiveInputEvent::Note(KeyboardTrigger {
                    generation,
                    source: None,
                    track,
                    transpose: 0.0,
                    velocity: 0.0,
                    note_off: true,
                }))
                .unwrap();
        }
        callback::render_audio_block(
            &mut data,
            &mut output,
            callback::AudioOutputPurpose::Playback,
        );
    }
    assert!(
        peak > 0.001,
        "saved instruments must render non-silent audio"
    );
    durations.sort_by(f64::total_cmp);
    eprintln!("live-audio 48k/512 queued pair -> rendered block: median={:.3}us p95={:.3}us max={:.3}us; onset stamp skew=0 samples; peak={peak}",
        durations[12], durations[23], durations[24]);
}
