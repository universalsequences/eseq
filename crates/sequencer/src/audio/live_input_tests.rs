use super::*;

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
