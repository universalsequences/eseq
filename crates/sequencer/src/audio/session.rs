//! Audio state construction shared by the device stream and isolated rendering.
//! This allocates fresh voice/event state without starting a device or scheduler.

use super::*;

pub(super) fn new_audio_callback_data(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
    master_recorder: Arc<MasterRecorder>,
    keyboard_rx: std::sync::mpsc::Receiver<crate::sequencer::LiveInputEvent>,
    bus_effect_runtime: Arc<Mutex<Arc<Vec<BusEffectRuntimeState>>>>,
    scheduled_events: Arc<ScheduledEventQueue<SCHEDULED_EVENT_QUEUE_CAPACITY>>,
    rendered_samples: Arc<AtomicU64>,
) -> Box<AudioCallbackData> {
    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> =
        (0..MAX_SAMPLER_POOLS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> = (0..MAX_INSTRUMENT_ENGINES)
        .map(|_| CustomEnginePool::new())
        .collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&state, t, &mut voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&state, t) {
            sync_custom_engine_pool(&state, engine_id, &mut custom_engine_pools[engine_id]);
        }
    }

    let initial_scheduler_snapshot_version = state.scheduler_snapshot_version();
    let initial_scheduler_snapshot = state.latest_scheduler_snapshot();
    let initial_num_tracks = initial_scheduler_snapshot.transport.num_tracks;
    let initial_topology_epoch = initial_scheduler_snapshot.transport.topology_epoch;
    let trace_audio = env_flag("TINYSEQ_AUDIO_TRACE", false);
    crate::instruments::voice_modulator::set_process_stats_enabled(trace_audio);
    if trace_audio {
        eprintln!("audio-trace: enabled");
    }

    // Keep the large callback state behind one pointer before handing the
    // closure through CPAL's generic stream builders. Passing it by value makes
    // debug builds reserve a copy-sized stack slot at every generic layer.
    Box::new(AudioCallbackData {
        lg: LiveGraphPtr(lg),
        state,
        num_channels,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        scheduler_snapshot: initial_scheduler_snapshot,
        scheduler_snapshot_version: initial_scheduler_snapshot_version,
        pressure: super::pressure::PressureState::new(),
        mono_held: (0..MAX_TRACKS).map(|_| MonoHeldNotes::default()).collect(),
        active_keyboard_notes: (0..MAX_TRACKS).map(|_| [None; MAX_VOICES]).collect(),
        keyboard_rx,
        master_recorder,
        accumulator_states: [crate::accumulator::AccumulatorRuntimeState::default(); MAX_TRACKS],
        last_playing: false,
        last_pattern: u32::MAX,
        last_num_tracks: initial_num_tracks,
        last_topology_epoch: initial_topology_epoch,
        pending_topology_delete_track: None,
        host_transport_clock: HostTransportClockRuntime::default(),
        free_patch_transport_routes: [FreePatchTransportRouteState::default(); MAX_TRACKS],
        rack_choke_last_trigger: [u64::MAX; MAX_TRACKS],
        rack_choke_note_offs: Vec::with_capacity(MAX_VOICES * 2),
        pending_accum_reset: [false; MAX_TRACKS],
        scheduled_events: Arc::clone(&scheduled_events),
        countdown_events: Vec::with_capacity(SCHEDULED_COUNTDOWN_CAPACITY),
        block_events: Vec::with_capacity(SCHEDULED_BLOCK_SCRATCH_CAPACITY),
        block_events_need_sort: false,
        current_callback_nframes: block_size,
        output_block_size: OutputBlockSizeVerifier::new(block_size),
        callback_thread_initialized: false,
        rendered_samples: Arc::clone(&rendered_samples),
        bus_effect_runtime,
        dropped_scheduled_events: 0,
        late_scheduled_events: 0,
        event_seq: 0,
        trace_audio,
        trace_callback_counter: 0,
        trace_render_probe_blocks: 0,
        trace_silent_active_callbacks: 0,
        transport_beats: 0.0,
        transport_was_playing: false,
        metronome: MetronomeState::default(),
        preview: preview::PreviewVoice::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_end_releases_gated_sampler_at_exact_frame_and_preserves_one_shot() {
        use crate::audiograph as graph;
        use crate::instruments::sampler::*;
        let engine = engine::init_headless_engine(48_000, 2).unwrap();
        let lg = engine.lg_ptr.0;
        let sample = vec![0.5_f32; 8192];
        let buffer = unsafe { graph::create_buffer(lg, 4096, 2, sample.as_ptr()) };
        assert!(buffer >= 0);
        let gated = create_sampler_node(lg, buffer, 48_000, "gated").unwrap();
        let one_shot = create_sampler_node(lg, buffer, 48_000, "one-shot").unwrap();
        unsafe {
            assert!(graph::graph_connect(lg, gated.node_id, 0, 0, 0));
            assert!(graph::graph_connect(lg, one_shot.node_id, 0, 0, 1));
        }
        let (_tx, rx) = std::sync::mpsc::channel();
        let mut data = new_audio_callback_data(
            lg, Arc::clone(&engine.state), 48_000, 2, 512,
            Arc::clone(&engine.master_recorder), rx,
            Arc::clone(&engine.buses.bus_effect_runtime),
            Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
        );
        let mut output = vec![0.0; 1024];
        unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
        for (pool, node, gate_mode) in [(0, &gated, 1.0), (1, &one_shot, 0.0)] {
            data.voice_pools[pool].add_voice(node.logical_id, node.node_id);
            data.voice_pools[pool].allocate_voice(60.0);
            let mut aux = [0.0; graph::GBE_AUX_CAP];
            aux[SAMPLER_EVENT_AUX_ENABLED] = 1.0;
            aux[SAMPLER_EVENT_AUX_VELOCITY] = 1.0;
            aux[SAMPLER_EVENT_AUX_SPEED] = 1.0;
            aux[SAMPLER_EVENT_AUX_GATE_SAMPLES] = 4096.0;
            aux[SAMPLER_EVENT_AUX_GATE_MODE] = gate_mode;
            aux[SAMPLER_EVENT_AUX_LOOP_MODE] = gate_mode;
            aux[SAMPLER_EVENT_AUX_RELEASE_SAMPLES] = 128.0;
            aux[SAMPLER_EVENT_AUX_END_POINT] = 1.0;
            aux[SAMPLER_EVENT_AUX_SR_HZ] = 48_000.0;
            let event = graph::GraphBlockEvent {
                logical_id: node.logical_id, frame_offset: 6, sequence: 0,
                kind: graph::GBE_NOTE_ON, aux_count: SAMPLER_EVENT_AUX_NOTE_ON_COUNT as u32, aux,
            };
            assert!(unsafe { graph::push_block_event(lg, event) });
        }
        schedule_gate_off_event(&mut data, 0, gated.logical_id, 6, 4096.0,
            GateOffTarget::Sampler { gatepitch_id: 0 });
        // The original release is far beyond this block. End cuts its gate
        // at frame 37 while retaining the independent ungated sample.
        dispatch_block_events_until(&mut data, 512, Some(549));
        unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 512); }
        assert!(output[..12].iter().all(|v| *v == 0.0));
        for frame in 6..37 { assert!(output[frame * 2] > 0.1); }
        let sustain = output[36 * 2];
        for frame in 37..512 {
            let expected = sustain * (1.0 - (frame - 36) as f32 / 128.0).max(0.0);
            assert!((output[frame * 2] - expected).abs() < 0.00001,
                "release frame {frame}: {} vs {expected}", output[frame * 2]);
            assert!(output[frame * 2 + 1] > 0.1, "one-shot frame {frame}");
        }
        assert!(!data.voice_pools[0].voices[0].active);
        assert!(data.voice_pools[1].voices[0].active);
        assert!(data.countdown_events.is_empty());
        assert_eq!(unsafe { graph::graph_block_event_delivery_failures(lg) }, 0);
        drop(data);
        unsafe { engine.destroy(); }
    }

    #[test]
    fn export_render_excludes_monitoring_and_live_recorder() {
        let engine = engine::init_headless_engine(48_000, 2).unwrap();
        let state = Arc::clone(&engine.state);
        state.transport.playing.store(true, Ordering::Relaxed);
        state.transport.metronome_enabled.store(true, Ordering::Relaxed);
        state.publish_scheduler_snapshot();
        let (_tx, rx) = std::sync::mpsc::channel();
        let mut data = new_audio_callback_data(
            engine.lg_ptr.0, state, 48_000, 2, 512,
            Arc::clone(&engine.master_recorder), rx,
            Arc::clone(&engine.buses.bus_effect_runtime),
            Arc::new(ScheduledEventQueue::new()), Arc::new(AtomicU64::new(0)),
        );
        preview::play(Arc::new(vec![0.25; 4096]), 48_000);
        engine.master_recorder.start().unwrap();
        let mut output = vec![0.0; 1024];
        render_audio_block(&mut data, &mut output, AudioOutputPurpose::Export { source_end_sample: u64::MAX });
        assert!(output.iter().all(|sample| *sample == 0.0));
        assert_eq!(data.rendered_samples.load(Ordering::Acquire), 512);
        assert_eq!(preview::position_seconds(), 0.0);
        assert!(engine.master_recorder.stop().unwrap().samples.is_empty());

        // The normal playback path still consumes preview and records the
        // pre-monitor master. Both use the same actual graph execution.
        engine.master_recorder.start().unwrap();
        audio_callback(&mut data, &mut output);
        assert!(output.iter().any(|sample| sample.abs() > 0.1));
        let take = engine.master_recorder.stop().unwrap();
        assert_eq!(take.samples.len(), 1024);
        assert!(take.samples.iter().all(|sample| *sample == 0.0));
        preview::stop();
        drop(data);
        unsafe { engine.destroy(); }
    }
}
